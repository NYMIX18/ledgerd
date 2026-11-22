<p align="center">
  <img src="docs/images/banner.svg" alt="ledgerd - a correctness-first double-entry ledger in Rust" width="100%">
</p>

<p align="center">
  <a href="https://github.com/NYMIX18/ledgerd/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/NYMIX18/ledgerd/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-stable-orange?logo=rust">
  <img alt="PostgreSQL" src="https://img.shields.io/badge/postgres-16-336791?logo=postgresql&logoColor=white">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-green">
</p>

**ledgerd** is a small payments ledger service. It records money movements as **double-entry transactions**, guarantees they are applied **exactly once** even when clients retry, and settles **M-Pesa (Safaricom Daraja) STK push callbacks** reliably even when they arrive duplicated, late, or out of order.

It is intentionally small and built to be *read*: every correctness decision is explicit, tested, and explained below.

> **Why this project exists.** Money is the domain where backend bugs are expensive: double charges, lost updates, race conditions, silent drift between "what the balance says" and "what actually happened". ledgerd is a compact, runnable answer to those problems, using Rust's type system where it helps and PostgreSQL's guarantees where they are stronger.

---


> **Dashboard:** a TypeScript/React front end with a Node backend-for-frontend lives in [`dashboard/`](dashboard/). Start everything with `docker compose up --build` and open http://localhost:3001.

## Table of contents

1. [What it does](#what-it-does)
2. [Architecture](#architecture)
3. [Double-entry in 30 seconds](#double-entry-in-30-seconds)
4. [How a transaction is posted](#how-a-transaction-is-posted)
5. [M-Pesa deposit flow](#m-pesa-deposit-flow)
6. [Data model](#data-model)
7. [Guarantees and how each is enforced](#guarantees-and-how-each-is-enforced)
8. [Failure modes](#failure-modes)
9. [API reference](#api-reference)
10. [Quick start](#quick-start)
11. [Testing](#testing)
12. [Benchmark](#benchmark)
13. [Design decisions and trade-offs](#design-decisions-and-trade-offs)
14. [Known limitations and roadmap](#known-limitations-and-roadmap)
15. [Project layout](#project-layout)

---

## What it does

| Capability | Summary |
|---|---|
| **Double-entry ledger** | Every transaction has 2-64 entries; total debits must equal total credits. Accounts have a kind (asset, liability, equity, revenue, expense) that decides which side increases them. |
| **Integer money** | Amounts are `i64` minor units (cents). There are no floats in the money path. A `Money`/`Currency` type refuses to mix currencies. |
| **Idempotent writes** | Every posting requires an `Idempotency-Key`. Same key + same request returns the original result; same key + *different* request is a `409`. |
| **Concurrency-safe balances** | Accounts are locked in a deterministic order (no deadlocks) and overdrafts are checked against locked rows (no lost updates, no double spends). |
| **Database-enforced invariants** | A deferred trigger rejects unbalanced transactions at `COMMIT`; ledger tables are append-only. Even a buggy or malicious query cannot corrupt the books. |
| **M-Pesa callbacks** | Webhook stores the raw callback and acks instantly. A background worker applies it atomically, with retries, backoff, and a dead state. |
| **Reconciliation** | Periodic and on-demand check that debits equal credits per currency and that every cached balance matches its entries. |
| **Observability** | Structured logs via `tracing`, Prometheus-style `/metrics`, `/healthz`. |

## Architecture

<p align="center">
  <img src="docs/images/architecture.svg" alt="System architecture diagram" width="100%">
</p>

```mermaid
flowchart LR
    subgraph Clients
        S["Your services"]
        D["Safaricom Daraja"]
    end

    subgraph ledgerd["ledgerd (one Rust binary)"]
        API["axum API<br/>auth, validation"]
        PE["Posting engine<br/>repo::post_transaction_tx"]
        WH["Webhook receiver<br/>token check, store, ack"]
        WK["Worker (tokio)<br/>callbacks + reconcile"]
    end

    DB[("PostgreSQL<br/>accounts, transactions, entries,<br/>payment_intents, mpesa_callbacks")]

    S -->|"Bearer + Idempotency-Key"| API --> PE --> DB
    D -->|"STK callback"| WH --> DB
    DB -->|"FOR UPDATE SKIP LOCKED"| WK
    WK --> PE
```

## Double-entry in 30 seconds

<p align="center">
  <img src="docs/images/double-entry.svg" alt="Double-entry illustration" width="90%">
</p>

- Every movement of money is recorded as **at least one debit and one credit of equal total**.
- Each account kind has a **normal side**: debit-normal kinds (asset, expense) go *up* on debits; credit-normal kinds (liability, equity, revenue) go *up* on credits.
- A customer's wallet balance is money **we owe them**, so wallets are *liabilities*. Receiving money into the M-Pesa paybill increases an *asset* (`MPESA_CLEARING`) and the customer's wallet liability by the same amount.
- Corrections are never edits. They are new, reversing transactions. The history is the source of truth.

## How a transaction is posted

```mermaid
sequenceDiagram
    autonumber
    participant C as Client
    participant A as axum API
    participant P as PostgreSQL
    C->>A: POST /v1/transactions + Idempotency-Key
    A->>A: validate (shape, amounts > 0, debits = credits)
    A->>P: BEGIN
    A->>P: INSERT transactions ... ON CONFLICT (idempotency_key) DO NOTHING
    alt key already used
        P-->>A: 0 rows inserted
        A->>P: SELECT request_hash
        alt same request
            A-->>C: 200 original result (Idempotent-Replayed: true)
        else different request
            A-->>C: 409 conflict
        end
    else new key
        A->>P: SELECT accounts WHERE id = ANY(..) ORDER BY id FOR UPDATE
        A->>A: currency match and overdraft checks on locked balances
        A->>P: INSERT entries, UPDATE cached balances
        A->>P: COMMIT (deferred trigger re-checks debits = credits)
        A-->>C: 201 Created
    end
```

Key points an engineer should notice:

- **Idempotency is claimed first**, inside the same DB transaction. A concurrent request with the same key *blocks on the unique index* until the first one commits or rolls back, then sees the result. (Covered by a test with 25 simultaneous identical requests.)
- **Locks are taken in `ORDER BY id`.** Two transfers going in opposite directions (A to B and B to A) cannot deadlock. (Covered by a 60-way concurrent test.)
- **Balance checks run on locked rows**, so two concurrent spends cannot both pass the check. (Covered: 30 concurrent spends of 100 against a 1,000 balance yields exactly 10 successes.)
- **A rejected request leaves no trace**, including its idempotency key, because everything is one transaction.

## M-Pesa deposit flow

```mermaid
sequenceDiagram
    autonumber
    participant App as Your backend
    participant L as ledgerd
    participant D as Daraja
    participant U as Customer phone
    participant W as Worker

    App->>L: POST /v1/deposits/mpesa (wallet, amount, msisdn)
    L->>L: create payment_intent (pending) with CheckoutRequestID
    Note over L,D: Production: call Daraja STK push here and store the CheckoutRequestID it returns
    D->>U: STK push prompt
    U->>D: enters PIN
    D->>L: POST /v1/webhooks/mpesa/{secret} (callback)
    L->>L: store raw callback (UNIQUE checkout id dedupes redeliveries)
    L-->>D: {"ResultCode":0,"ResultDesc":"Accepted"} (fast ack)
    W->>L: claim due callback (SKIP LOCKED)
    W->>W: match intent, check amount, post ledger txn
    Note over W: One DB transaction: ledger entries + intent=completed + callback=processed
```

### Callback lifecycle

```mermaid
stateDiagram-v2
    [*] --> received: webhook stored
    received --> processed: ledger posted atomically
    received --> received: error, retry with backoff (2s, 4s, 8s ... max 5 min)
    received --> dead: attempts exhausted
    processed --> [*]
    dead --> [*]: needs a human
```

What this handles:

| Situation | Behaviour |
|---|---|
| Daraja sends the same callback twice | The second insert hits `UNIQUE(checkout_request_id)` and is ignored. |
| Callback arrives before our intent row is visible | Processing fails with "intent not found", the callback is retried with exponential backoff, then succeeds. |
| Customer cancels / insufficient funds (`ResultCode != 0`) | Intent is marked `failed`; the wallet is never credited. |
| Callback amount differs from what we requested | Intent is marked `failed` (`amount_mismatch`); the wallet is never credited. |
| Worker crashes mid-processing | The DB transaction rolls back; the row is still `received` and is picked up again. |
| Several worker instances | `FOR UPDATE SKIP LOCKED` lets them share the queue without double-processing. |
| Callback never matches anything | After `MAX_CALLBACK_ATTEMPTS` it becomes `dead` and increments a metric. |

> **Security note.** Daraja does **not** sign callbacks, so there is no HMAC to verify. ledgerd protects the webhook with an unguessable secret path segment (constant-time compare), and, more importantly, **never trusts the callback on its own**: it must match a payment intent *we* created, and the amount must match. In production also allow-list Safaricom's IP ranges at your proxy.

## Data model

```mermaid
erDiagram
    ACCOUNTS ||--o{ ENTRIES : "debited/credited by"
    TRANSACTIONS ||--|{ ENTRIES : "contains"
    ACCOUNTS ||--o{ PAYMENT_INTENTS : "receives deposits"
    PAYMENT_INTENTS ||--o| MPESA_CALLBACKS : "settled by checkout_request_id"

    ACCOUNTS {
        uuid id PK
        text code UK
        text kind "asset|liability|equity|revenue|expense"
        text currency
        bigint balance_minor "cached, natural balance"
        bool allow_negative
    }
    TRANSACTIONS {
        uuid id PK
        text idempotency_key UK
        text request_hash "sha256 of canonical request"
        text description
        text external_ref "e.g. M-Pesa receipt"
    }
    ENTRIES {
        bigserial id PK
        uuid transaction_id FK
        uuid account_id FK
        text direction "debit|credit"
        bigint amount_minor "> 0"
    }
    PAYMENT_INTENTS {
        uuid id PK
        text checkout_request_id UK
        uuid account_id FK
        bigint amount_minor
        text status "pending|completed|failed"
    }
    MPESA_CALLBACKS {
        uuid id PK
        text checkout_request_id UK
        jsonb payload
        text status "received|processed|dead"
        int attempts
        timestamptz next_attempt_at
    }
```

## Guarantees and how each is enforced

| Guarantee | Enforced by |
|---|---|
| Every transaction balances | (1) pure validation in `NewTransaction::validate`, (2) a **deferred constraint trigger** that re-checks at `COMMIT`. Test: `database_itself_rejects_unbalanced_entries` writes raw SQL and is rejected. |
| History cannot be rewritten | `BEFORE UPDATE OR DELETE` triggers on `entries` and `transactions`. Test: `ledger_rows_are_append_only`. |
| Exactly-once posting per key | `UNIQUE(idempotency_key)` + `ON CONFLICT DO NOTHING` + request fingerprint. |
| No overdraft under concurrency | Row locks (`FOR UPDATE`) + check against locked balance + `CHECK (allow_negative OR balance_minor >= 0)` as a last line of defence. |
| No deadlocks between opposing transfers | Locks always acquired in `ORDER BY id`. |
| No currency mixing | Account currency is compared to the transaction currency under lock. |
| Cached balances never drift | Updated in the same transaction as entries; `reconcile()` recomputes them from entries and reports differences. |
| Callback + ledger atomicity | `post_transaction_tx` takes the *caller's* DB transaction, so the worker posts and updates status in one commit. |

## Failure modes

| What goes wrong | What happens |
|---|---|
| Client times out and retries | Same `Idempotency-Key` returns the original result with `Idempotent-Replayed: true`. No double charge. |
| Client reuses a key for a different payment | `409 conflict`. The fingerprint (order-independent SHA-256 of the request) differs. |
| Two requests race on the same key | One wins; the other blocks on the unique index, then replays. |
| App crashes between "debit" and "credit" | Impossible by construction: both are in one DB transaction. |
| Someone runs a manual `UPDATE` on ledger rows | Rejected by the append-only trigger. |
| Someone inserts unbalanced entries by hand | Rejected at `COMMIT` by the deferred trigger. |
| Postgres restarts mid-request | The transaction aborts; the client retries safely with the same key. |
| Reconciliation finds drift | `/v1/reconcile` returns `500` with the offending account ids; `ledger_reconcile_*` gauges go non-zero; an `ERROR` log is emitted. |

## API reference

All `/v1/*` endpoints except the webhook require `Authorization: Bearer $API_KEY`. Amounts are integer **minor units** (`2500` = KES 25.00).

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/accounts` | Create an account |
| `GET` | `/v1/accounts/{id}` | Read an account and its balance |
| `POST` | `/v1/transactions` | Post a transaction (**requires** `Idempotency-Key`) |
| `GET` | `/v1/transactions/{id}` | Read a transaction with its entries |
| `POST` | `/v1/deposits/mpesa` | Start an M-Pesa deposit (creates a payment intent) |
| `POST` | `/v1/webhooks/mpesa/{token}` | Daraja callback receiver (secret path, no Bearer) |
| `GET` | `/v1/reconcile` | Run reconciliation now |
| `GET` | `/healthz` | Liveness + DB check |
| `GET` | `/metrics` | Prometheus text metrics |

**Post a transfer**

```bash
curl -s -X POST localhost:8080/v1/transactions \
  -H "Authorization: Bearer $API_KEY" \
  -H "Idempotency-Key: pay-42" \
  -H "Content-Type: application/json" \
  -d '{
    "description": "Alice pays Bob",
    "currency": "KES",
    "entries": [
      { "account_id": "<alice-wallet-id>", "direction": "debit",  "amount_minor": 2500 },
      { "account_id": "<bob-wallet-id>",   "direction": "credit", "amount_minor": 2500 }
    ]
  }'
```

Errors share one shape:

```json
{ "error": { "code": "insufficient_funds", "message": "insufficient funds in account 3f2c..." } }
```

| HTTP | `code` | Meaning |
|---|---|---|
| 401 | `unauthorized` | Missing/invalid Bearer token or webhook token |
| 404 | `not_found` | Unknown account / transaction |
| 409 | `conflict` | Duplicate account code, or idempotency key reused with a different request |
| 422 | `validation_error` | Malformed, unbalanced, cross-currency, missing `Idempotency-Key` |
| 422 | `insufficient_funds` | Debit would take a non-overdraft account below zero |
| 500 | `internal_error` | Details are logged, never returned |

## Quick start

### Option A: Docker Compose

```bash
docker compose up --build
# API on http://localhost:8080
./scripts/demo.sh        # needs curl + jq
```

### Option B: local Rust + Postgres

```bash
cp .env.example .env     # then edit secrets
make db                  # Postgres 16 via docker compose (or use your own)
make run                 # applies migrations automatically on start
./scripts/demo.sh
```

The demo walks through: creating accounts, funding, a transfer sent twice with the same key (only applied once), a rejected overdraft, an M-Pesa deposit with a duplicated callback, reconciliation, and metrics.

### Configuration

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string |
| `API_KEY` | required | Bearer token for `/v1/*` |
| `MPESA_WEBHOOK_TOKEN` | required | Secret path segment of the Daraja callback URL |
| `BIND_ADDR` | `0.0.0.0:8080` | Listen address |
| `WORKER_INTERVAL_MS` | `1000` | Worker poll interval |
| `MAX_CALLBACK_ATTEMPTS` | `8` | Retries before a callback is marked `dead` |
| `RUST_LOG` | `info` | Log filter |

## Testing

```bash
# Postgres must be reachable; each integration test creates (and drops) its own database.
DATABASE_URL=postgres://ledger:ledger@localhost:5432/postgres cargo test
```

32 tests: 17 unit + 15 integration against real Postgres (via `#[sqlx::test]`).

| Area | Test |
|---|---|
| Correctness | transfers move balances; reconcile is clean afterwards |
| Idempotency | replay returns the same transaction; key reuse with a different body conflicts; **25 concurrent identical requests post exactly once** |
| Concurrency | **60 opposing concurrent transfers: no deadlock, total conserved**; **30 concurrent spends cannot overdraw** |
| DB invariants | raw SQL cannot commit unbalanced entries; UPDATE/DELETE on ledger rows is rejected |
| M-Pesa | duplicate callbacks credit once; callback-before-intent is retried then succeeds; unmatched callback goes `dead`; failed payment and amount mismatch never credit |
| HTTP | full request path: auth, validation, idempotent replay header, webhook token, reconcile |
| Pure logic | money arithmetic and overflow, entry validation, order-independent fingerprint, Daraja payload parsing |

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy -D warnings`, and the full test suite against a Postgres service on every push.

## Benchmark

A dependency-free benchmark exercises the posting path (SQL + algorithm, no HTTP) with unique idempotency keys:

```bash
DATABASE_URL=postgres://ledger:ledger@localhost:5432/ledger \
N=20000 C=32 ACCOUNTS=1000 cargo run --release --example bench
```

It reports throughput and p50/p95/p99 latency for two scenarios: **contended** (every request touches the same two accounts, so row locks serialise them) and **spread** (random pairs across many accounts).

Results depend heavily on hardware, Postgres settings and network, so run it on your own machine before quoting numbers.

The gap between the two scenarios is the honest cost of strong consistency on hot accounts; see the trade-offs below.

## Design decisions and trade-offs

**Postgres does the hard part.** Row locks, unique indexes, deferred triggers and CHECK constraints provide stronger guarantees than application-level locking, and they hold across multiple app instances. The application adds a fast path (validate early) and clear errors.

**Cached balance + entries, reconciled.** Reading a balance is O(1) because `accounts.balance_minor` is maintained transactionally. The cost: a hot account serialises writers (visible in the contended benchmark). Mitigations if this became real: shard hot accounts into sub-accounts, or compute balances from entries asynchronously. `reconcile()` exists precisely because a cache needs an auditor.

**READ COMMITTED + explicit row locks** instead of SERIALIZABLE. Locks are explicit and ordered, so behaviour is predictable and there are no serialization-failure retry loops. SERIALIZABLE would be simpler to reason about but needs retry handling on every write.

**Idempotency via the database, not a cache.** A Redis `SETNX` can lose the key on eviction or fail between "check" and "write". Here the key, the transaction and the entries commit together or not at all.

**Request fingerprint on the key.** A key alone only proves "this was seen". Pairing it with a canonical hash catches the dangerous bug where a client reuses a key for a different payment.

**Store-then-process webhooks.** Daraja retries if you are slow or erroring. Persisting the raw payload and acking immediately decouples Daraja's timeout from our processing, gives a durable audit trail, and makes retries/backoff ours to control.

**`post_transaction_tx` takes a transaction.** This composition point is what lets the worker update the intent, post to the ledger and mark the callback processed in a single atomic commit.

**Runtime-checked SQL (`sqlx::query`) rather than compile-time macros.** Keeps the build independent of a live database and avoids committing `.sqlx` offline metadata; the integration suite against real Postgres is what catches SQL mistakes.

**Hand-rolled metrics.** Atomic counters rendered in Prometheus text format keep the dependency list short. For a larger service use the `metrics` or `prometheus` crate.

## Known limitations and roadmap

Being explicit about what this is *not*:

- **STK push is simulated.** `POST /v1/deposits/mpesa` mints a `CheckoutRequestID` locally. A production version calls Daraja's OAuth + STK push endpoints at that point (the spot is marked in `api.rs`). The callback handling is real and tested against Daraja's documented payload shape.
- **Single static API key.** Real systems want per-client keys/scopes and rotation.
- **No reversal/refund endpoint yet.** The append-only design is built for it (a reversal is a new transaction); the endpoint is not written.
- **No pagination or statement endpoint** (list entries per account).
- **No dead-letter admin tooling.** `dead` callbacks are visible in the table and metrics, but there is no replay endpoint.
- **Hot-account contention** as described above.
- **Not load-tested at scale** and not security-audited. Treat it as a well-tested reference implementation, not a drop-in payments core.

Ideas for next steps: account statement + cursor pagination, reversal endpoint, outbox table for emitting events, OpenTelemetry traces, per-account sharding strategy, signed API keys.

## Project layout

```
.
├── migrations/0001_init.sql   Schema, deferred balance trigger, append-only triggers
├── src/
│   ├── main.rs                Wiring: config, pool, migrations, worker, graceful shutdown
│   ├── api.rs                 axum router, handlers, auth middleware
│   ├── repo.rs                All SQL. post_transaction_tx is the heart of the service
│   ├── domain.rs              Account kinds, entry validation, request fingerprint
│   ├── money.rs               Integer Money + Currency types
│   ├── mpesa.rs               Daraja payload parsing, constant-time token check
│   ├── worker.rs              Callback processor (SKIP LOCKED, backoff) + reconciliation loop
│   ├── metrics.rs             Prometheus-style counters
│   ├── config.rs / error.rs   Env config; error type -> HTTP mapping
│   └── lib.rs
├── tests/integration.rs       15 tests against real Postgres
├── examples/bench.rs          Posting-path benchmark
├── scripts/demo.sh            curl walkthrough
├── docs/images/               Architecture and double-entry diagrams (SVG + PNG)
├── docs/DESIGN_NOTES.md       Design rationale, trade-offs and known limitations
├── Dockerfile, docker-compose.yml, Makefile
└── .github/workflows/ci.yml
```

## License

MIT. See [LICENSE](LICENSE).

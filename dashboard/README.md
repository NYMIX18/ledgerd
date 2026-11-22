# ledgerd dashboard

A TypeScript/React front end for ledgerd, with a small Node backend-for-frontend (BFF).

```
browser (React + TanStack Query + Zod)
   │  /api/*  (no secrets)
   ▼
BFF (Fastify + Zod)  ── holds LEDGERD_API_KEY, validates every response ──▶  ledgerd (Rust, :8080)
```

## Why a BFF

- **The API key stays on the server.** The browser never sees it.
- **Contract checking at both hops.** `shared/schemas.ts` mirrors ledgerd's Rust types. The BFF validates what
  ledgerd returns and the browser validates what the BFF returns. If ledgerd's shape drifts, the BFF answers
  `502 upstream_contract_violation` instead of the UI rendering `NaN`.
- **A truly concurrent demo.** Browsers cap parallel connections per host (about 6 on HTTP/1.1), so a browser can
  never put 25+ identical requests in flight at once. The idempotency demo runs in the BFF, where it can.

## Run it

```bash
# from the repo root: db + api + dashboard
docker compose up --build
# open http://localhost:3001
```

Local development (ledgerd running on :8080):

```bash
cd dashboard
npm install
npm run dev          # BFF on :3001, Vite on :5173
npm run typecheck
npm test             # schema + BFF unit tests (no network needed)
npm run e2e          # Playwright, needs the full stack running
```

## Screens

| Tab | What it does |
|---|---|
| Accounts | Create accounts, add existing ones by id, see live balances |
| Post a transaction | Two-entry transfer with exact decimal parsing (no floats), idempotency key shown, option to reuse it and see a replay |
| Idempotency | Sends N identical requests at once with one key and shows exactly how many were created vs replayed |
| Integrity | Runs reconciliation: recomputed balances and debits = credits per currency |

## Known limitations

- Ledgerd has no list endpoints yet, so account ids are remembered in `localStorage`. `GET /v1/accounts`,
  `GET /v1/transactions` (paginated) and `GET /v1/callbacks` are the planned additions; `useAccounts()` is the only
  place that changes.
- Transfers are two-entry. The schema already supports up to 64 entries; the form does not.
- No user authentication on the dashboard itself. Put it behind your own auth before exposing it.

See [docs/BREAK_IT.md](docs/BREAK_IT.md) for cross-layer bugs worth reproducing.

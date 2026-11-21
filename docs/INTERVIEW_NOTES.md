# Interview notes

Talking points for walking someone through ledgerd. Short answers first, depth available on request.

## 60-second pitch

"ledgerd is a double-entry ledger service in Rust. The interesting part is correctness under failure: every write is idempotent, balances are protected by ordered row locks, and the database itself rejects unbalanced or edited ledger rows. It also settles M-Pesa callbacks reliably: store raw, ack fast, apply asynchronously and atomically with retries. I tested the nasty cases against real Postgres: 25 concurrent identical requests, opposing concurrent transfers, concurrent spends against one balance."

## Questions to expect

**Why double-entry instead of just a balance column?**
It gives an audit trail and a built-in checksum: debits must equal credits, so lost or invented money is detectable. A bare balance column can be silently wrong with no way to prove it.

**How do you prevent double charging on retries?**
Mandatory `Idempotency-Key`, claimed with `INSERT ... ON CONFLICT DO NOTHING` inside the same DB transaction as the postings. A request fingerprint distinguishes a true retry from key reuse with a different body.

**What if two requests with the same key arrive simultaneously?**
The second blocks on the unique index until the first commits (then replays) or rolls back (then proceeds). Tested with 25 concurrent requests: exactly one posting.

**How do you avoid deadlocks?**
Accounts are locked with `SELECT ... ORDER BY id FOR UPDATE`, so every transaction acquires locks in the same global order.

**Why READ COMMITTED and not SERIALIZABLE?**
Explicit ordered locks make behaviour predictable without serialization-failure retry loops. The trade-off is that correctness depends on taking the right locks, which is why the invariants are also enforced by DB constraints and triggers as a backstop.

**What if the app has a bug?**
The deferred trigger rejects unbalanced transactions at COMMIT; append-only triggers block edits; a CHECK constraint blocks negative balances on non-overdraft accounts; reconciliation detects cache drift.

**Why store the callback before processing?**
Daraja retries on slow/failed responses. Persisting and acking immediately decouples their timeout from our work, dedupes via a unique key, and lets us retry with backoff.

**What about callbacks that arrive before the intent exists?**
Processing errors with "intent not found", the row is rescheduled with exponential backoff, and succeeds once the intent is visible. After the retry limit it goes `dead` and is surfaced via metrics.

**Daraja callbacks aren't signed. How do you trust them?**
We don't, alone. Secret path token (constant-time compare), IP allow-listing at the edge, and the callback must match a payment intent we created with the same amount. A forged callback with a wrong id or amount credits nothing.

**Where would this break at scale?**
Hot accounts: the cached balance serialises writers. Options: sub-account sharding, async balance projection, batching. The contended vs spread benchmark shows the gap.

**How would you add refunds?**
A reversal is a new transaction with opposite entries referencing the original (`external_ref`), under its own idempotency key. Append-only storage already supports it.

**Why no compile-time checked SQL?**
Keeps builds independent of a live database and avoids offline metadata. The real-Postgres integration suite catches SQL regressions. Switching to `query!` is a mechanical change if the team prefers it.

## Honest caveats to volunteer

- STK push is simulated (callback handling is real).
- Single static API key.
- No reversal endpoint, pagination, or dead-letter replay yet.
- Benchmarked on a dev machine, not production hardware.

# Break it: cross-layer bugs

Each entry is a bug that is easy to introduce in this stack, how it shows up, how to trace it across the layers,
and the fix. Reproduce each one yourself before you trust it; the "trace" steps are the point.

## 1. Money parsed with floats

**Inject:** in `PostPanel`, replace `parseMajorToMinor(amount)` with `Math.round(parseFloat(amount) * 100)`, or drop
the `Math.round` and send the raw product.

**Symptom:** certain amounts (for example `0.29`) are posted as 28 or fail with a 422 about a non-integer amount.

**Trace:** browser network tab shows `amount_minor: 28.999999999999996` → BFF `NewTransaction` rejects it with
`amount_minor: Expected integer` → the error code never reaches ledgerd. With the rounding bug instead of the
missing one, ledgerd happily posts 28 minor units and the books are wrong by one cent.

**Fix:** parse the decimal string, never multiply floats (`parseMajorToMinor`). Covered by `shared/schemas.test.ts`.

## 2. Schema drift upstream

**Inject:** rename `balance_minor` to `balance` in ledgerd's `Account` struct.

**Symptom without validation:** balances render as `NaN` or blank.

**Trace:** ledgerd JSON → BFF `relay()` runs `S.Account.safeParse` → fails → BFF returns
`502 upstream_contract_violation` naming the field → the UI shows a clear error.

**Fix:** update `shared/schemas.ts` with the rust change in the same commit. Covered by `server/app.test.ts`.

## 3. Stale balances after a write

**Inject:** remove `store.refresh()` from `PostPanel`'s `onSettled`.

**Symptom:** the transaction succeeds but the Accounts tab shows the old balance until a manual reload (TanStack
Query serves the cached value within `staleTime`).

**Trace:** network tab shows no `GET /api/accounts/:id` after the POST → query cache key `["account", id]` was never
invalidated.

**Fix:** invalidate in `onSettled` (success **and** error), because a failed request can still have changed state
elsewhere, for example a concurrent request.

## 4. Browser-side concurrency demo

**Inject:** move the idempotency demo into the browser with `Promise.all(fetch(...) × 25)`.

**Symptom:** the demo "passes" but proves little: the browser queues requests behind its per-host connection limit,
so ledgerd mostly sees them one after another and the race the demo is meant to show barely exists.

**Trace:** the network tab waterfall shows requests staggered in groups of about 6 instead of all starting together.

**Fix:** run the burst from the BFF (`POST /api/demo/idempotency`), as implemented.

## 5. A new Idempotency-Key per retry

**Inject:** call `crypto.randomUUID()` inside `mutationFn` instead of holding the key in state.

**Symptom:** a double-click or a network retry posts the money twice. Each request has a fresh key, so ledgerd
correctly treats them as different requests.

**Trace:** two POSTs with different `Idempotency-Key` headers and two transaction ids in the same second.

**Fix:** generate the key once per *intent* (when the form is opened or after a success) and reuse it across
retries of that intent, as `PostPanel` does.

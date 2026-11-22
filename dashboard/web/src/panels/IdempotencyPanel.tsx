import { useState, type FormEvent } from "react";
import { useMutation } from "@tanstack/react-query";
import { IdempotencyDemoResult, parseMajorToMinor } from "../../../shared/schemas";
import { api, post } from "../api";
import type { useAccounts } from "../accounts";
import { AccountSelect, ErrorNote, NeedAccounts } from "./common";

export function IdempotencyPanel({ store }: { store: ReturnType<typeof useAccounts> }) {
  const { accounts } = store;
  const [debit, setDebit] = useState("");
  const [credit, setCredit] = useState("");
  const [amount, setAmount] = useState("10.00");
  const [count, setCount] = useState(25);
  const [localError, setLocalError] = useState<string | null>(null);

  const run = useMutation({
    mutationFn: async () => {
      const body = {
        debit_account_id: debit,
        credit_account_id: credit,
        amount_minor: parseMajorToMinor(amount)!,
        count,
      };
      return (await api("/demo/idempotency", IdempotencyDemoResult, post(body))).data;
    },
    onSettled: () => void store.refresh(),
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setLocalError(null);
    if (!debit || !credit || debit === credit) return setLocalError("Choose two different accounts.");
    if (parseMajorToMinor(amount) === null) return setLocalError("Enter a positive amount with at most 2 decimals.");
    run.mutate();
  };

  const r = run.data;
  const exactlyOnce = r && r.distinct_transaction_ids.length === 1 && r.created === 1 && r.failed === 0;

  return (
    <section>
      <h2>Idempotency under concurrency</h2>
      <p className="lede">
        Fire the same request many times at once, all with one Idempotency-Key. A correct ledger applies it exactly
        once and answers every other request with the stored result.
      </p>
      <NeedAccounts count={accounts.length} />
      <form onSubmit={submit} className="form-grid">
        <AccountSelect label="Debit account" accounts={accounts} value={debit} onChange={setDebit} />
        <AccountSelect label="Credit account" accounts={accounts} value={credit} onChange={setCredit} />
        <label className="field">
          <span>Amount per request</span>
          <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} required />
        </label>
        <label className="field">
          <span>Identical requests: {count}</span>
          <input type="range" min={2} max={100} value={count} onChange={(e) => setCount(Number(e.target.value))} />
        </label>
        <div>
          <button type="submit" disabled={run.isPending}>
            {run.isPending ? "Sending" : `Send ${count} at once`}
          </button>
        </div>
      </form>
      {localError && (
        <p role="alert" className="note note-error">
          {localError}
        </p>
      )}
      <ErrorNote error={run.error} />

      {r && (
        <div className="tally" data-testid="tally" role="status">
          <p className="tally-headline" data-testid="tally-headline">
            {r.requested} requests, {r.distinct_transaction_ids.length}{" "}
            {r.distinct_transaction_ids.length === 1 ? "transaction" : "transactions"}
          </p>
          <p className={exactlyOnce ? "verdict verdict-ok" : "verdict verdict-bad"}>
            {exactlyOnce
              ? `Applied exactly once. ${r.replayed} replayed in ${r.elapsed_ms} ms.`
              : `Unexpected result: ${r.created} created, ${r.replayed} replayed, ${r.failed} failed.`}
          </p>
          <ol className="strip" aria-label="Outcome of each request">
            {Array.from({ length: r.requested }, (_, i) => {
              const kind = i < r.created ? "created" : i < r.created + r.replayed ? "replayed" : "failed";
              return <li key={i} className={`tick tick-${kind}`} style={{ animationDelay: `${i * 18}ms` }} title={kind} />;
            })}
          </ol>
          <p className="legend">
            <span className="tick tick-created" /> created ({r.created})
            <span className="tick tick-replayed" /> replayed ({r.replayed})
            <span className="tick tick-failed" /> failed ({r.failed})
          </p>
          <p className="muted">
            Key <code>{r.key}</code>
          </p>
        </div>
      )}
    </section>
  );
}

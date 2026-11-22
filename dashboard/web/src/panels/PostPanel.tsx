import { useState, type FormEvent } from "react";
import { useMutation } from "@tanstack/react-query";
import { TransactionView, parseMajorToMinor, formatMinor } from "../../../shared/schemas";
import { api, post } from "../api";
import type { useAccounts } from "../accounts";
import { AccountSelect, ErrorNote, NeedAccounts } from "./common";

export function PostPanel({ store }: { store: ReturnType<typeof useAccounts> }) {
  const { accounts } = store;
  const [debit, setDebit] = useState("");
  const [credit, setCredit] = useState("");
  const [amount, setAmount] = useState("");
  const [description, setDescription] = useState("");
  const [key, setKey] = useState(() => crypto.randomUUID());
  const [reuseKey, setReuseKey] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);

  const debitAcc = accounts.find((a) => a.id === debit);
  const creditAcc = accounts.find((a) => a.id === credit);

  const send = useMutation({
    mutationFn: async () => {
      const minor = parseMajorToMinor(amount)!;
      const body = {
        description,
        currency: debitAcc!.currency,
        entries: [
          { account_id: debit, direction: "debit", amount_minor: minor },
          { account_id: credit, direction: "credit", amount_minor: minor },
        ],
      };
      return api("/transactions", TransactionView, post(body, { "idempotency-key": key }));
    },
    onSettled: () => {
      void store.refresh(); // balances changed (or may have): never show a stale one
      if (!reuseKey) setKey(crypto.randomUUID());
    },
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setLocalError(null);
    if (!debitAcc || !creditAcc) return setLocalError("Choose both accounts.");
    if (debit === credit) return setLocalError("Debit and credit accounts must differ.");
    if (debitAcc.currency !== creditAcc.currency) return setLocalError("Both accounts must use the same currency.");
    if (parseMajorToMinor(amount) === null) return setLocalError("Enter a positive amount with at most 2 decimals.");
    send.mutate();
  };

  const result = send.data;

  return (
    <section>
      <h2>Post a transaction</h2>
      <p className="lede">
        Every posting is two entries that must balance. A debit increases asset and expense accounts and decreases the
        rest; a credit does the opposite.
      </p>
      <NeedAccounts count={accounts.length} />
      <form onSubmit={submit} className="form-grid">
        <AccountSelect label="Debit account" accounts={accounts} value={debit} onChange={setDebit} />
        <AccountSelect label="Credit account" accounts={accounts} value={credit} onChange={setCredit} />
        <label className="field">
          <span>Amount{debitAcc ? ` (${debitAcc.currency})` : ""}</span>
          <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="250.00" required />
        </label>
        <label className="field">
          <span>Description</span>
          <input value={description} onChange={(e) => setDescription(e.target.value)} maxLength={500} required />
        </label>
        <div className="field wide">
          <span>Idempotency key</span>
          <code className="key">{key}</code>
          <label className="check">
            <input type="checkbox" checked={reuseKey} onChange={(e) => setReuseKey(e.target.checked)} />
            Keep this key for the next submit (send the same request twice to see a replay)
          </label>
        </div>
        <div>
          <button type="submit" disabled={send.isPending}>
            {send.isPending ? "Posting" : "Post transaction"}
          </button>
        </div>
      </form>

      {localError && (
        <p role="alert" className="note note-error">
          {localError}
        </p>
      )}
      <ErrorNote error={send.error} />

      {result && (
        <div className={`receipt ${result.replayed ? "receipt-replayed" : "receipt-posted"}`} role="status">
          <strong>{result.replayed ? "Replayed" : "Posted"}</strong>
          <span>
            {result.replayed
              ? "This key was already used with an identical request, so nothing was applied again."
              : "Applied once."}
          </span>
          <dl>
            <dt>Transaction</dt>
            <dd>{result.data.id}</dd>
            {result.data.entries.map((en, i) => (
              <div key={i} className="entry-line">
                <dt>{en.direction}</dt>
                <dd>{formatMinor(en.amount_minor, result.data.currency)}</dd>
              </div>
            ))}
          </dl>
        </div>
      )}
    </section>
  );
}

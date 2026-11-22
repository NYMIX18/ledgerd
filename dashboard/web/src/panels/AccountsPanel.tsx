import { useState, type FormEvent } from "react";
import { useMutation } from "@tanstack/react-query";
import { Account, AccountKind, Currency, formatMinor } from "../../../shared/schemas";
import { api, post } from "../api";
import type { useAccounts } from "../accounts";
import { ErrorNote } from "./common";

export function AccountsPanel({ store }: { store: ReturnType<typeof useAccounts> }) {
  const [code, setCode] = useState("");
  const [name, setName] = useState("");
  const [kind, setKind] = useState<AccountKind>("liability");
  const [currency, setCurrency] = useState<Currency>("KES");
  const [allowNegative, setAllowNegative] = useState(false);
  const [existingId, setExistingId] = useState("");

  const create = useMutation({
    mutationFn: async () =>
      (await api("/accounts", Account, post({ code, name, kind, currency, allow_negative: allowNegative }))).data,
    onSuccess: (acc) => {
      store.add(acc.id);
      setCode("");
      setName("");
    },
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    create.mutate();
  };

  return (
    <section>
      <h2>Accounts</h2>
      {store.ids.length === 0 ? (
        <p className="note">No accounts yet. Create one below, or paste the id of an existing account.</p>
      ) : (
        <table className="ledger">
          <thead>
            <tr>
              <th>Code</th>
              <th>Kind</th>
              <th className="num">Balance</th>
              <th>
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {store.queries.map((q, i) => {
              const id = store.ids[i]!;
              const a = q.data;
              return (
                <tr key={id}>
                  <td>{a ? a.code : <span className="muted">{q.isError ? "unavailable" : "loading"}</span>}</td>
                  <td>{a?.kind ?? ""}</td>
                  <td className="num">{a ? formatMinor(a.balance_minor, a.currency) : ""}</td>
                  <td className="actions">
                    <button className="link" onClick={() => store.remove(id)}>
                      Forget
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <h3>Create an account</h3>
      <form onSubmit={submit} className="form-grid">
        <label className="field">
          <span>Code</span>
          <input value={code} onChange={(e) => setCode(e.target.value)} maxLength={64} required />
        </label>
        <label className="field">
          <span>Name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} required />
        </label>
        <label className="field">
          <span>Kind</span>
          <select value={kind} onChange={(e) => setKind(e.target.value as AccountKind)}>
            {AccountKind.options.map((k) => (
              <option key={k}>{k}</option>
            ))}
          </select>
        </label>
        <label className="field">
          <span>Currency</span>
          <select value={currency} onChange={(e) => setCurrency(e.target.value as Currency)}>
            {Currency.options.map((c) => (
              <option key={c}>{c}</option>
            ))}
          </select>
        </label>
        <label className="check">
          <input type="checkbox" checked={allowNegative} onChange={(e) => setAllowNegative(e.target.checked)} />
          Allow negative balance (for source accounts such as equity)
        </label>
        <div>
          <button type="submit" disabled={create.isPending}>
            {create.isPending ? "Creating" : "Create account"}
          </button>
        </div>
      </form>
      <ErrorNote error={create.error} />

      <h3>Add an existing account</h3>
      <form
        className="inline"
        onSubmit={(e) => {
          e.preventDefault();
          store.add(existingId.trim());
          setExistingId("");
        }}
      >
        <input
          aria-label="Account id"
          placeholder="Account UUID"
          value={existingId}
          onChange={(e) => setExistingId(e.target.value)}
          pattern="[0-9a-fA-F-]{36}"
          required
        />
        <button type="submit">Add</button>
      </form>
    </section>
  );
}

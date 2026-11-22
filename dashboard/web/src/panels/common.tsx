import type { ReactNode } from "react";
import type { Account } from "../../../shared/schemas";
import { formatMinor } from "../../../shared/schemas";
import { ApiClientError } from "../api";

export function AccountSelect(props: {
  label: string;
  accounts: Account[];
  value: string;
  onChange: (id: string) => void;
}) {
  return (
    <label className="field">
      <span>{props.label}</span>
      <select value={props.value} onChange={(e) => props.onChange(e.target.value)} required>
        <option value="">Choose an account</option>
        {props.accounts.map((a) => (
          <option key={a.id} value={a.id}>
            {a.code} ({a.kind}, {formatMinor(a.balance_minor, a.currency)})
          </option>
        ))}
      </select>
    </label>
  );
}

export function ErrorNote({ error }: { error: unknown }): ReactNode {
  if (!error) return null;
  const e = error instanceof ApiClientError ? error : null;
  return (
    <p role="alert" className="note note-error">
      {e ? <strong>{e.code}</strong> : null} {e ? e.message : String(error)}
    </p>
  );
}

export function NeedAccounts({ count, min = 2 }: { count: number; min?: number }) {
  if (count >= min) return null;
  return (
    <p className="note">
      You need at least {min} accounts. Create them in the Accounts tab (a funded wallet plus a counter-account that
      allows negative balances works well).
    </p>
  );
}

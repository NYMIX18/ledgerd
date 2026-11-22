import { useMutation } from "@tanstack/react-query";
import { ReconcileReport } from "../../../shared/schemas";
import { api } from "../api";
import { ErrorNote } from "./common";

export function IntegrityPanel() {
  const check = useMutation({ mutationFn: async () => (await api("/reconcile", ReconcileReport)).data });
  const r = check.data;

  return (
    <section>
      <h2>Integrity</h2>
      <p className="lede">
        Reconciliation recomputes every balance from the entries and checks that total debits equal total credits in
        each currency. Both lists should always be empty.
      </p>
      <button onClick={() => check.mutate()} disabled={check.isPending}>
        {check.isPending ? "Checking" : "Run reconciliation"}
      </button>
      <ErrorNote error={check.error} />
      {r && (
        <div className={`receipt ${r.ok ? "receipt-posted" : "receipt-failed"}`} role="status">
          <strong>{r.ok ? "Books balance" : "Discrepancy found"}</strong>
          <dl>
            <dt>Unbalanced currencies</dt>
            <dd>{r.unbalanced_currencies.length ? r.unbalanced_currencies.join(", ") : "none"}</dd>
            <dt>Accounts with drifted balances</dt>
            <dd>{r.drifted_accounts.length ? r.drifted_accounts.join(", ") : "none"}</dd>
          </dl>
        </div>
      )}
    </section>
  );
}

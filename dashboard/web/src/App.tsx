import { useState } from "react";
import { useAccounts } from "./accounts";
import { AccountsPanel } from "./panels/AccountsPanel";
import { PostPanel } from "./panels/PostPanel";
import { IdempotencyPanel } from "./panels/IdempotencyPanel";
import { IntegrityPanel } from "./panels/IntegrityPanel";

const TABS = [
  { id: "accounts", label: "Accounts" },
  { id: "post", label: "Post a transaction" },
  { id: "idempotency", label: "Idempotency" },
  { id: "integrity", label: "Integrity" },
] as const;
type TabId = (typeof TABS)[number]["id"];

export function App() {
  const [tab, setTab] = useState<TabId>("accounts");
  const store = useAccounts();

  return (
    <div className="shell">
      <header className="masthead">
        <h1>ledgerd</h1>
        <p>A double-entry ledger that stays correct when requests are retried, raced and replayed.</p>
      </header>

      <div role="tablist" aria-label="Sections" className="tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            id={`tab-${t.id}`}
            aria-selected={tab === t.id}
            aria-controls={`panel-${t.id}`}
            className="tab"
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      <main role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`}>
        {tab === "accounts" && <AccountsPanel store={store} />}
        {tab === "post" && <PostPanel store={store} />}
        {tab === "idempotency" && <IdempotencyPanel store={store} />}
        {tab === "integrity" && <IntegrityPanel />}
      </main>
    </div>
  );
}

// Loads sample data into a running ledgerd. Needs only Node 18+ (no npm install).
//   node seed.mjs
// Optional: set LEDGERD_URL / LEDGERD_API_KEY if yours differ from the defaults.

const BASE = process.env.LEDGERD_URL ?? "http://127.0.0.1:8080";
const KEY = process.env.LEDGERD_API_KEY ?? "change-me-dev-key";
const RUN = Date.now().toString(36).toUpperCase(); // keeps account codes unique per run

async function call(method, path, body, extraHeaders = {}) {
  let res;
  try {
    res = await fetch(`${BASE}/v1${path}`, {
      method,
      headers: { authorization: `Bearer ${KEY}`, "content-type": "application/json", ...extraHeaders },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(10_000),
    });
  } catch (e) {
    console.error(`\nCould not reach ledgerd at ${BASE}. Is the API running? (${e.message})`);
    process.exit(1);
  }
  const text = await res.text();
  let json = null;
  try { json = text ? JSON.parse(text) : null; } catch {}
  return { status: res.status, json, replayed: res.headers.get("idempotent-replayed") === "true" };
}

const kes = (minor) => `KES ${(minor / 100).toLocaleString("en-US", { minimumFractionDigits: 2 })}`;

async function account(code, name, kind, allowNegative = false) {
  const r = await call("POST", "/accounts", { code: `${code}_${RUN}`, name, kind, currency: "KES", allow_negative: allowNegative });
  if (r.status !== 201) { console.error("Failed to create", code, r.status, r.json); process.exit(1); }
  console.log(`  created ${r.json.code.padEnd(22)} ${kind.padEnd(9)} ${r.json.id}`);
  return r.json;
}

// debit one account, credit another, same amount
async function transfer(label, key, debit, credit, amountMinor) {
  const r = await call("POST", "/transactions", {
    description: label, currency: "KES",
    entries: [
      { account_id: debit.id, direction: "debit", amount_minor: amountMinor },
      { account_id: credit.id, direction: "credit", amount_minor: amountMinor },
    ],
  }, { "idempotency-key": `seed-${RUN}-${key}` });
  const outcome = r.status === 201 ? "posted" : r.status === 200 && r.replayed ? "replayed" : `HTTP ${r.status} ${r.json?.error?.code ?? ""}`;
  console.log(`  ${label.padEnd(34)} ${kes(amountMinor).padStart(14)}  ->  ${outcome}`);
  return r;
}

console.log(`\nSeeding ledgerd at ${BASE} (run ${RUN})\n\nAccounts`);
const source = await account("SOURCE_KES", "Funding source (equity)", "equity", true);
const alice  = await account("ALICE_WALLET", "Alice wallet", "liability");
const bob    = await account("BOB_WALLET", "Bob wallet", "liability");
const fees   = await account("FEES_REVENUE", "Fee income", "revenue");

console.log("\nTransactions");
await transfer("Fund Alice wallet", "fund-alice", source, alice, 500_000);
await transfer("Fund Bob wallet", "fund-bob", source, bob, 200_000);
await transfer("Alice pays Bob", "alice-bob", alice, bob, 75_000);
await transfer("Platform fee from Bob", "bob-fee", bob, fees, 2_500);

console.log("\nIdempotency: send 'Alice pays Bob' again with the SAME key");
await transfer("Alice pays Bob (retry)", "alice-bob", alice, bob, 75_000);

console.log("\nGuard rail: Bob tries to send more than he has");
await transfer("Bob overspends", "bob-overspend", bob, alice, 9_999_999);

console.log("\nBalances");
for (const a of [source, alice, bob, fees]) {
  const r = await call("GET", `/accounts/${a.id}`);
  const bal = r.json.balance_minor;
  console.log(`  ${r.json.code.padEnd(22)} ${(bal < 0 ? "-" : "") + kes(Math.abs(bal))}`);
}

const rec = await call("GET", "/reconcile");
console.log(`\nReconciliation: ${rec.json?.ok ? "books balance" : "PROBLEM " + JSON.stringify(rec.json)}`);

const ids = [source.id, alice.id, bob.id, fees.id];
console.log(`
To see these in the dashboard: open http://localhost:5173, press F12, open the Console tab,
paste the line below, and press Enter.

localStorage.setItem("ledgerd.accounts", JSON.stringify(${JSON.stringify(ids)})); location.reload();
`);

import { describe, expect, it } from "vitest";
import { buildApp } from "./app";

process.env.NODE_ENV = "test";

const ID_A = "11111111-1111-4111-8111-111111111111";
const ID_B = "22222222-2222-4222-8222-222222222222";
const TXN = "33333333-3333-4333-8333-333333333333";

const account = {
  id: ID_A,
  code: "WALLET_A",
  name: "Wallet A",
  kind: "liability",
  currency: "KES",
  balance_minor: 5000,
  allow_negative: false,
  created_at: "2026-11-22T10:00:00Z",
};

const txnView = {
  id: TXN,
  idempotency_key: "k",
  description: "demo",
  currency: "KES",
  external_ref: null,
  created_at: "2026-11-22T10:00:00Z",
  entries: [
    { account_id: ID_A, direction: "debit", amount_minor: 100 },
    { account_id: ID_B, direction: "credit", amount_minor: 100 },
  ],
};

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", ...headers } });

const make = (fetchImpl: typeof fetch) =>
  buildApp({ ledgerdUrl: "http://ledgerd.test", apiKey: "secret", fetchImpl });

describe("BFF", () => {
  it("adds the API key server-side and never exposes it", async () => {
    let seenAuth = "";
    const app = make((async (_u: string, init?: RequestInit) => {
      seenAuth = String((init?.headers as Record<string, string>).authorization);
      return json(200, account);
    }) as unknown as typeof fetch);
    const res = await app.inject({ method: "GET", url: `/api/accounts/${ID_A}` });
    expect(res.statusCode).toBe(200);
    expect(seenAuth).toBe("Bearer secret");
    expect(res.body).not.toContain("secret");
  });

  it("answers 502 when Ledgerd breaks the contract instead of passing bad data on", async () => {
    const drifted = { ...account, balance_minor: undefined, balance: 5000 }; // field renamed upstream
    const app = make((async () => json(200, drifted)) as unknown as typeof fetch);
    const res = await app.inject({ method: "GET", url: `/api/accounts/${ID_A}` });
    expect(res.statusCode).toBe(502);
    expect(res.json().error.code).toBe("upstream_contract_violation");
  });

  it("requires an Idempotency-Key and rejects unbalanced transactions locally", async () => {
    const app = make((async () => json(500, {})) as unknown as typeof fetch);
    const payload = {
      description: "x",
      currency: "KES",
      entries: [
        { account_id: ID_A, direction: "debit", amount_minor: 100 },
        { account_id: ID_B, direction: "credit", amount_minor: 99 },
      ],
    };
    const noKey = await app.inject({ method: "POST", url: "/api/transactions", payload });
    expect(noKey.statusCode).toBe(422);
    const unbalanced = await app.inject({
      method: "POST",
      url: "/api/transactions",
      payload,
      headers: { "idempotency-key": "abc" },
    });
    expect(unbalanced.statusCode).toBe(422);
    expect(unbalanced.json().error.message).toContain("debits must equal credits");
  });

  it("idempotency demo fires N identical requests with ONE key and tallies created vs replayed", async () => {
    const keys = new Set<string>();
    let posts = 0;
    const app = make((async (url: string, init?: RequestInit) => {
      if (init?.method === "GET") return json(200, account);
      posts += 1;
      keys.add(String((init?.headers as Record<string, string>)["idempotency-key"]));
      // first request creates, the rest replay – what a correct Ledgerd does
      return posts === 1 ? json(201, txnView) : json(200, txnView, { "idempotent-replayed": "true" });
    }) as unknown as typeof fetch);
    const res = await app.inject({
      method: "POST",
      url: "/api/demo/idempotency",
      payload: { debit_account_id: ID_A, credit_account_id: ID_B, amount_minor: 100, count: 25 },
    });
    expect(res.statusCode).toBe(200);
    const out = res.json();
    expect(keys.size).toBe(1);
    expect(out.requested).toBe(25);
    expect(out.created).toBe(1);
    expect(out.replayed).toBe(24);
    expect(out.failed).toBe(0);
    expect(out.distinct_transaction_ids).toEqual([TXN]);
  });
});

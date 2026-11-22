import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import path from "node:path";
import Fastify, { type FastifyInstance, type FastifyReply } from "fastify";
import fastifyStatic from "@fastify/static";
import { z, type ZodTypeAny } from "zod";
import * as S from "../shared/schemas";

export interface AppOptions {
  ledgerdUrl: string;
  apiKey: string;
  /** Injectable for tests. Defaults to global fetch. */
  fetchImpl?: typeof fetch;
  /** Built web app to serve in production. */
  staticDir?: string;
}

interface Upstream {
  status: number;
  json: unknown;
  replayed: boolean;
}

const errorBody = (code: string, message: string): S.ApiErrorBody => ({ error: { code, message } });

export function buildApp(opts: AppOptions): FastifyInstance {
  const app = Fastify({ logger: process.env.NODE_ENV !== "test" });
  const doFetch = opts.fetchImpl ?? fetch;

  /** Every call to Ledgerd goes through here. The API key lives only on the server. */
  async function ledgerd(
    method: "GET" | "POST",
    p: string,
    body?: unknown,
    extraHeaders: Record<string, string> = {},
  ): Promise<Upstream> {
    const res = await doFetch(`${opts.ledgerdUrl}/v1${p}`, {
      method,
      headers: {
        authorization: `Bearer ${opts.apiKey}`,
        "content-type": "application/json",
        ...extraHeaders,
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await res.text();
    let json: unknown = null;
    try {
      json = text ? JSON.parse(text) : null;
    } catch {
      /* non-JSON body: leave null, caller reports a contract violation */
    }
    return { status: res.status, json, replayed: res.headers.get("idempotent-replayed") === "true" };
  }

  /**
   * Relay an upstream answer. 2xx bodies are validated against `schema`; if Ledgerd ever
   * returns something the contract does not describe we answer 502 rather than pass it on.
   */
  function relay(reply: FastifyReply, up: Upstream, schema: ZodTypeAny, okStatuses: number[] = [200, 201, 202]) {
    if (okStatuses.includes(up.status)) {
      const parsed = schema.safeParse(up.json);
      if (!parsed.success) {
        return reply
          .code(502)
          .send(errorBody("upstream_contract_violation", `Ledgerd response did not match the contract: ${parsed.error.issues[0]?.path.join(".") || "(root)"} ${parsed.error.issues[0]?.message ?? ""}`.trim()));
      }
      if (up.replayed) reply.header("idempotent-replayed", "true");
      return reply.code(up.status).send(parsed.data);
    }
    const err = S.ApiErrorBody.safeParse(up.json);
    if (err.success) return reply.code(up.status).send(err.data);
    return reply.code(502).send(errorBody("upstream_error", `Unexpected status ${up.status} from Ledgerd`));
  }

  const invalid = (reply: FastifyReply, e: z.ZodError) =>
    reply.code(422).send(errorBody("validation_error", e.issues.map((i) => `${i.path.join(".") || "body"}: ${i.message}`).join("; ")));

  // ---- health -------------------------------------------------------------
  app.get("/api/health", async () => ({ status: "ok" }));

  // ---- accounts -----------------------------------------------------------
  app.post("/api/accounts", async (req, reply) => {
    const body = S.NewAccount.safeParse(req.body);
    if (!body.success) return invalid(reply, body.error);
    return relay(reply, await ledgerd("POST", "/accounts", body.data), S.Account);
  });

  app.get<{ Params: { id: string } }>("/api/accounts/:id", async (req, reply) => {
    const id = z.string().uuid().safeParse(req.params.id);
    if (!id.success) return invalid(reply, id.error);
    return relay(reply, await ledgerd("GET", `/accounts/${id.data}`), S.Account);
  });

  // ---- transactions -------------------------------------------------------
  app.post("/api/transactions", async (req, reply) => {
    const key = z.string().trim().min(1).max(255).safeParse(req.headers["idempotency-key"]);
    if (!key.success) return reply.code(422).send(errorBody("validation_error", "Idempotency-Key header (1-255 chars) is required"));
    const body = S.NewTransaction.safeParse(req.body);
    if (!body.success) return invalid(reply, body.error);
    const up = await ledgerd("POST", "/transactions", body.data, { "idempotency-key": key.data });
    return relay(reply, up, S.TransactionView);
  });

  app.get<{ Params: { id: string } }>("/api/transactions/:id", async (req, reply) => {
    const id = z.string().uuid().safeParse(req.params.id);
    if (!id.success) return invalid(reply, id.error);
    return relay(reply, await ledgerd("GET", `/transactions/${id.data}`), S.TransactionView);
  });

  // ---- M-Pesa deposit intent (STK push is simulated by Ledgerd) -------------
  app.post("/api/deposits/mpesa", async (req, reply) => {
    const body = S.NewDeposit.safeParse(req.body);
    if (!body.success) return invalid(reply, body.error);
    return relay(reply, await ledgerd("POST", "/deposits/mpesa", body.data), S.PaymentIntent);
  });

  // ---- integrity ----------------------------------------------------------
  // Ledgerd answers 500 *with a report body* when it finds drift, so both 200 and 500 are valid here.
  app.get("/api/reconcile", async (_req, reply) => {
    const up = await ledgerd("GET", "/reconcile");
    const report = S.ReconcileReport.safeParse(up.json);
    if (report.success) return reply.code(200).send(report.data);
    return relay(reply, up, S.ReconcileReport);
  });

  // ---- idempotency demo ---------------------------------------------------
  // Runs on the server on purpose: a browser caps parallel connections per host (about 6 on
  // HTTP/1.1), so it could never put 25+ identical requests in flight at once. Node can.
  app.post("/api/demo/idempotency", async (req, reply) => {
    const body = S.IdempotencyDemoRequest.safeParse(req.body);
    if (!body.success) return invalid(reply, body.error);
    const d = body.data;

    const acct = await ledgerd("GET", `/accounts/${d.debit_account_id}`);
    const account = S.Account.safeParse(acct.json);
    if (!account.success) return relay(reply, acct, S.Account);

    const txn: S.NewTransaction = {
      description: `idempotency demo x${d.count}`,
      currency: account.data.currency,
      entries: [
        { account_id: d.debit_account_id, direction: "debit", amount_minor: d.amount_minor },
        { account_id: d.credit_account_id, direction: "credit", amount_minor: d.amount_minor },
      ],
    };
    const key = `demo-${randomUUID()}`;
    const started = performance.now();
    const results = await Promise.all(
      Array.from({ length: d.count }, () => ledgerd("POST", "/transactions", txn, { "idempotency-key": key })),
    );

    const statuses: Record<string, number> = {};
    const ids = new Set<string>();
    for (const r of results) {
      statuses[String(r.status)] = (statuses[String(r.status)] ?? 0) + 1;
      const t = S.TransactionView.safeParse(r.json);
      if (t.success) ids.add(t.data.id);
    }
    const result: S.IdempotencyDemoResult = {
      key,
      requested: d.count,
      created: statuses["201"] ?? 0,
      replayed: statuses["200"] ?? 0,
      failed: d.count - (statuses["201"] ?? 0) - (statuses["200"] ?? 0),
      statuses,
      distinct_transaction_ids: [...ids],
      elapsed_ms: Math.round(performance.now() - started),
    };
    return reply.send(result);
  });

  // ---- static web app (production build) ------------------------------------
  if (opts.staticDir && existsSync(opts.staticDir)) {
    void app.register(fastifyStatic, { root: path.resolve(opts.staticDir) });
  }

  return app;
}

/**
 * Single source of truth for the wire format between browser <-> BFF <-> Ledgerd.
 * The shapes mirror ledgerd's Rust types exactly (src/domain.rs, src/repo.rs, src/money.rs).
 * Both hops validate with these schemas, so a drifting API fails loudly instead of rendering NaN.
 *
 * Money is integer minor units (cents). Ledgerd uses i64; JSON numbers are exact up to 2^53,
 * far beyond any realistic balance here, so `number` is safe. Never use floats for money.
 */
import { z } from "zod";

export const Currency = z.enum(["KES", "USD", "EUR"]);
export type Currency = z.infer<typeof Currency>;

export const AccountKind = z.enum(["asset", "liability", "equity", "revenue", "expense"]);
export type AccountKind = z.infer<typeof AccountKind>;

export const Direction = z.enum(["debit", "credit"]);
export type Direction = z.infer<typeof Direction>;

const Uuid = z.string().uuid();

// ---- accounts ---------------------------------------------------------------

export const Account = z.object({
  id: Uuid,
  code: z.string(),
  name: z.string(),
  kind: AccountKind,
  currency: Currency,
  balance_minor: z.number().int(),
  allow_negative: z.boolean(),
  created_at: z.string(),
});
export type Account = z.infer<typeof Account>;

export const NewAccount = z.object({
  code: z.string().trim().min(1).max(64),
  name: z.string().trim().min(1),
  kind: AccountKind,
  currency: Currency,
  allow_negative: z.boolean().default(false),
});
export type NewAccount = z.input<typeof NewAccount>;

// ---- transactions -----------------------------------------------------------

export const Entry = z.object({
  account_id: Uuid,
  direction: Direction,
  amount_minor: z.number().int().positive(),
});
export type Entry = z.infer<typeof Entry>;

/** Mirrors NewTransaction::validate() so the UI rejects bad input before a round trip. */
export const NewTransaction = z
  .object({
    description: z.string().trim().min(1).max(500),
    currency: Currency,
    entries: z.array(Entry).min(2).max(64),
    external_ref: z.string().nullish(),
  })
  .superRefine((t, ctx) => {
    const sum = (d: Direction) =>
      t.entries.filter((e) => e.direction === d).reduce((n, e) => n + e.amount_minor, 0);
    if (sum("debit") !== sum("credit")) {
      ctx.addIssue({ code: "custom", message: "debits must equal credits", path: ["entries"] });
    }
  });
export type NewTransaction = z.infer<typeof NewTransaction>;

export const TransactionView = z.object({
  id: Uuid,
  idempotency_key: z.string(),
  description: z.string(),
  currency: Currency,
  external_ref: z.string().nullable(),
  created_at: z.string(),
  entries: z.array(Entry),
});
export type TransactionView = z.infer<typeof TransactionView>;

// ---- M-Pesa / reconciliation ------------------------------------------------

export const NewDeposit = z.object({
  account_id: Uuid,
  amount_minor: z.number().int().positive(),
  msisdn: z.string().regex(/^254\d{9}$/, "must look like 2547XXXXXXXX"),
});

export const PaymentIntent = z.object({
  id: Uuid,
  checkout_request_id: z.string(),
  account_id: Uuid,
  amount_minor: z.number().int(),
  msisdn: z.string(),
  status: z.string(),
});

export const ReconcileReport = z.object({
  ok: z.boolean(),
  unbalanced_currencies: z.array(z.string()),
  drifted_accounts: z.array(Uuid),
});
export type ReconcileReport = z.infer<typeof ReconcileReport>;

// ---- errors -----------------------------------------------------------------

export const ApiErrorBody = z.object({
  error: z.object({ code: z.string(), message: z.string() }),
});
export type ApiErrorBody = z.infer<typeof ApiErrorBody>;

// ---- idempotency demo (BFF-only endpoint) -----------------------------------

export const IdempotencyDemoRequest = z.object({
  debit_account_id: Uuid,
  credit_account_id: Uuid,
  amount_minor: z.number().int().positive(),
  count: z.number().int().min(2).max(100),
});
export type IdempotencyDemoRequest = z.infer<typeof IdempotencyDemoRequest>;

export const IdempotencyDemoResult = z.object({
  key: z.string(),
  requested: z.number().int(),
  created: z.number().int(), // HTTP 201: this request posted the transaction
  replayed: z.number().int(), // HTTP 200: this request got the stored result
  failed: z.number().int(),
  statuses: z.record(z.string(), z.number().int()),
  distinct_transaction_ids: z.array(Uuid),
  elapsed_ms: z.number(),
});
export type IdempotencyDemoResult = z.infer<typeof IdempotencyDemoResult>;

// ---- money helpers (no floats anywhere) -------------------------------------

/** "12.5" -> 1250. Returns null for anything that is not a plain amount with <= 2 decimals. */
export function parseMajorToMinor(input: string): number | null {
  const m = /^(\d+)(?:\.(\d{1,2}))?$/.exec(input.trim());
  if (!m) return null;
  const whole = Number(m[1]);
  const frac = Number((m[2] ?? "").padEnd(2, "0"));
  const minor = whole * 100 + frac;
  return Number.isSafeInteger(minor) && minor > 0 ? minor : null;
}

export function formatMinor(minor: number, currency: Currency): string {
  const sign = minor < 0 ? "-" : "";
  const abs = Math.abs(minor);
  const whole = Math.floor(abs / 100).toLocaleString("en-US");
  const frac = String(abs % 100).padStart(2, "0");
  return `${sign}${currency} ${whole}.${frac}`;
}

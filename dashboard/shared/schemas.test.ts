import { describe, expect, it } from "vitest";
import { Account, NewTransaction, formatMinor, parseMajorToMinor } from "./schemas";

const A = "11111111-1111-4111-8111-111111111111";
const B = "22222222-2222-4222-8222-222222222222";

describe("money helpers", () => {
  it("parses amounts exactly, without floating point", () => {
    expect(parseMajorToMinor("12.5")).toBe(1250);
    expect(parseMajorToMinor("0.10")).toBe(10);
    expect(parseMajorToMinor("0.29")).toBe(29); // 0.29 * 100 === 28.999999999999996 with floats
    expect(parseMajorToMinor("1000")).toBe(100000);
  });
  it("rejects junk, zero, negatives and sub-cent precision", () => {
    for (const bad of ["", "abc", "0", "-5", "1.234", "1,000", "1e3"]) {
      expect(parseMajorToMinor(bad)).toBeNull();
    }
  });
  it("formats minor units", () => {
    expect(formatMinor(123456, "KES")).toBe("KES 1,234.56");
    expect(formatMinor(-5, "USD")).toBe("-USD 0.05");
  });
});

describe("schemas", () => {
  it("NewTransaction enforces double entry", () => {
    const base = { description: "t", currency: "KES" as const };
    const ok = NewTransaction.safeParse({
      ...base,
      entries: [
        { account_id: A, direction: "debit", amount_minor: 100 },
        { account_id: B, direction: "credit", amount_minor: 100 },
      ],
    });
    const bad = NewTransaction.safeParse({
      ...base,
      entries: [
        { account_id: A, direction: "debit", amount_minor: 100 },
        { account_id: B, direction: "credit", amount_minor: 90 },
      ],
    });
    expect(ok.success).toBe(true);
    expect(bad.success).toBe(false);
  });
  it("Account rejects an unknown currency", () => {
    const r = Account.safeParse({
      id: A, code: "X", name: "X", kind: "asset", currency: "GBP",
      balance_minor: 0, allow_negative: false, created_at: "now",
    });
    expect(r.success).toBe(false);
  });
});

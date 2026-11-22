import { expect, test, type APIRequestContext } from "@playwright/test";

async function createAccount(
  request: APIRequestContext,
  body: { code: string; name: string; kind: string; currency: string; allow_negative: boolean },
) {
  const res = await request.post("/api/accounts", { data: body });
  expect(res.status()).toBe(201);
  return (await res.json()) as { id: string };
}

test("25 identical concurrent requests post exactly once, and the UI shows it", async ({ page, request }) => {
  const run = Date.now().toString(36);
  const source = await createAccount(request, {
    code: `E2E_SRC_${run}`, name: "e2e source", kind: "equity", currency: "KES", allow_negative: true,
  });
  const wallet = await createAccount(request, {
    code: `E2E_WAL_${run}`, name: "e2e wallet", kind: "liability", currency: "KES", allow_negative: false,
  });

  await page.addInitScript(
    (ids) => localStorage.setItem("ledgerd.accounts", JSON.stringify(ids)),
    [source.id, wallet.id],
  );
  await page.goto("/");
  await page.getByRole("tab", { name: "Idempotency" }).click();

  await page.getByLabel("Debit account").selectOption(source.id);
  await page.getByLabel("Credit account").selectOption(wallet.id);
  await page.getByLabel("Identical requests: 25").waitFor();
  await page.getByRole("button", { name: "Send 25 at once" }).click();

  await expect(page.getByTestId("tally-headline")).toHaveText("25 requests, 1 transaction");
  await expect(page.getByText(/Applied exactly once/)).toBeVisible();

  // The wallet was credited exactly once: 10.00 KES, not 250.00.
  const res = await request.get(`/api/accounts/${wallet.id}`);
  expect((await res.json()).balance_minor).toBe(1000);
});

test("an overdraft is rejected with a readable error", async ({ page, request }) => {
  const run = Date.now().toString(36);
  const a = await createAccount(request, {
    code: `E2E_A_${run}`, name: "a", kind: "liability", currency: "KES", allow_negative: false,
  });
  const b = await createAccount(request, {
    code: `E2E_B_${run}`, name: "b", kind: "liability", currency: "KES", allow_negative: false,
  });
  await page.addInitScript((ids) => localStorage.setItem("ledgerd.accounts", JSON.stringify(ids)), [a.id, b.id]);
  await page.goto("/");
  await page.getByRole("tab", { name: "Post a transaction" }).click();
  await page.getByLabel("Debit account").selectOption(a.id);
  await page.getByLabel("Credit account").selectOption(b.id);
  await page.getByLabel(/^Amount/).fill("5.00");
  await page.getByLabel("Description").fill("should fail");
  await page.getByRole("button", { name: "Post transaction" }).click();
  await expect(page.getByRole("alert")).toContainText("insufficient_funds");
});

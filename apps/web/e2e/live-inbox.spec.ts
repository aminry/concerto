import { expect, test } from "@playwright/test";

// LIVE UI-E2E (Task 523): the web inbox showing a REAL notification fetched from
// a running Core over the connect-web bridge (proxied same-origin by vite). Only
// runs when CONCERTO_LIVE=1 with a Core booted on the bridge + a seeded
// notification (see scripts/web-live-demo.sh); skipped in normal CI.
test.skip(!process.env.CONCERTO_LIVE, "requires a live Core on the bridge (CONCERTO_LIVE=1)");

test("renders a live notification from a running Core", async ({ page }) => {
  await page.goto("/");
  // Point at the dev origin so connect-web posts same-origin → vite proxy → bridge.
  await page.getByTestId("core-url").fill("http://127.0.0.1:5174");
  await page.getByTestId("connect").click();
  await expect(page.getByTestId("notification").first()).toBeVisible({ timeout: 20_000 });
  await page.screenshot({ path: "e2e/__screenshots__/inbox-live.png", fullPage: true });
});

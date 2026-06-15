import { expect, test } from "@playwright/test";

// UI-E2E for the notifications inbox SPA (Task 519). These run without a live
// Core (idle + connection-error states); the live-data flow (real notifications
// over the connect-web bridge) is exercised in 520/523's harness against a
// spawned Core. Screenshots are written for visual review.

test("renders the idle connect state", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("Concerto", { exact: true })).toBeVisible();
  await expect(page.getByTestId("core-url")).toBeVisible();
  await expect(page.getByTestId("connect")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Notifications" })).toBeVisible();
  await expect(page.getByTestId("unread-toggle")).toBeVisible();
  await expect(page.getByTestId("idle")).toBeVisible();
  await page.screenshot({ path: "e2e/__screenshots__/inbox-idle.png", fullPage: true });
});

test("shows an error banner when the Core is unreachable", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("connect").click();
  await expect(page.getByTestId("error")).toBeVisible({ timeout: 20_000 });
  await page.screenshot({ path: "e2e/__screenshots__/inbox-error.png", fullPage: true });
});

test("unread-only toggle is interactive", async ({ page }) => {
  await page.goto("/");
  const toggle = page.getByTestId("unread-toggle");
  await expect(toggle).not.toBeChecked();
  await toggle.check();
  await expect(toggle).toBeChecked();
});

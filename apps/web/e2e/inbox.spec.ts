import { expect, test } from "@playwright/test";

// The `?mock=1` harness (src/lib/mock-setup.ts) installs this driver on window.
declare global {
  interface Window {
    __mock?: {
      pushLive: (id: string, title: string) => void;
      failStream: () => void;
      handle: { addNotification: (init: { id: string; title: string }) => number };
    };
  }
}

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

// ── Task 520: live updates over the mock DataClient (no real Core). ──────────
// `?mock=1` installs a Core-free DataClient (@concerto/client/testing) + a
// `window.__mock` driver; we connect, then PUSH a `notification.events` frame
// and assert the new card appears with NO manual refresh.

test("live: a streamed notification appears without a manual refresh", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.getByTestId("connect").click();

  // Seeded inbox loads + the live badge shows we're on the stream.
  await expect(page.getByTestId("notification")).toHaveCount(2);
  await expect(page.getByTestId("live-status")).toHaveAttribute("data-live", "live");

  // Push a live notification through the mock stream (no UI interaction).
  await page.evaluate(() => window.__mock!.pushLive("live-99", "Agent crashed in bee"));

  // It shows up at the TOP of the feed — without clicking Connect again.
  await expect(page.getByText("Agent crashed in bee")).toBeVisible({ timeout: 10_000 });
  await expect(page.getByTestId("notification")).toHaveCount(3);
  await expect(page.getByTestId("notification").first()).toContainText("Agent crashed in bee");

  await page.screenshot({ path: "e2e/__screenshots__/inbox-live-stream.png", fullPage: true });
});

test("live: falls back to polling when the stream errors", async ({ page }) => {
  await page.goto("/?mock=1");
  await page.getByTestId("connect").click();

  // Wait for the seeded feed first (the `?mock=1` chunk compiles on first hit
  // under the dev server) so the live badge is reliably mounted.
  await expect(page.getByTestId("notification")).toHaveCount(2);
  await expect(page.getByTestId("live-status")).toHaveAttribute("data-live", "live", {
    timeout: 10_000,
  });

  // Add a notification to the backing inbox, THEN kill the stream. The polling
  // fallback re-fetches GetInbox and surfaces the new row.
  await page.evaluate(() => {
    window.__mock!.handle.addNotification({ id: "poll-1", title: "Schedule run finished" });
    window.__mock!.failStream();
  });

  await expect(page.getByTestId("live-status")).toHaveAttribute("data-live", "polling", {
    timeout: 10_000,
  });
  await expect(page.getByText("Schedule run finished")).toBeVisible({ timeout: 10_000 });

  await page.screenshot({ path: "e2e/__screenshots__/inbox-polling.png", fullPage: true });
});

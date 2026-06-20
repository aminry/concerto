import { expect, test } from "@playwright/test";

// Ephemeral browser pairing UI-E2E (Task 522). Runs over the `?mock=1` Core-free
// DataClient (no real Core) — we only exercise the CLIENT-SIDE session machinery:
// Connect mints an 8h `web_ephemeral` cert into IndexedDB; "remember browser"
// controls whether it survives a reload (clear-on-close vs persist).
//
// TIER-3: the Core actually TRUSTING the stub-signed cert needs the real phone
// signer (Task 511) + the bridge auth middleware; this spec proves the full
// client session lifecycle without it.

test.describe("ephemeral browser pairing (Task 522)", () => {
  test("Connect mints a session; the status chip shows Paired", async ({ page }) => {
    await page.goto("/?mock=1");

    // Pre-Connect: not paired.
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "none");

    await page.getByTestId("connect").click();

    // After Connect: a session is minted + the chip flips to "paired".
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "paired", {
      timeout: 10_000,
    });
    await expect(page.getByTestId("session-status")).toContainText("Paired");
    await expect(page.getByTestId("session-status")).toContainText("expires in");

    await page.screenshot({ path: "e2e/__screenshots__/session-paired.png", fullPage: true });
  });

  test("reload WITHOUT remember loses the session (cleared on tab close)", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByTestId("connect").click();
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "paired", {
      timeout: 10_000,
    });
    // remember-browser is OFF by default.
    await expect(page.getByTestId("remember-browser")).not.toBeChecked();

    // Reloading fires pagehide → clear-on-close wipes the IndexedDB session.
    await page.reload();
    await page.waitForSelector('[data-testid="session-status"]');
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "none");
  });

  test("reload WITH remember keeps the session", async ({ page }) => {
    await page.goto("/?mock=1");

    // Opt IN to remember-browser BEFORE connecting so the minted session persists.
    await page.getByTestId("remember-browser").check();
    await expect(page.getByTestId("remember-browser")).toBeChecked();

    await page.getByTestId("connect").click();
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "paired", {
      timeout: 10_000,
    });
    const expiryText = await page.getByTestId("session-status").textContent();

    // Reload: clear-on-close is disarmed (remember=ON) so the session is restored
    // from IndexedDB on boot — still "paired".
    await page.reload();
    await page.waitForSelector('[data-testid="session-status"]');
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "paired", {
      timeout: 10_000,
    });
    // The remember preference also survived (localStorage) so the box stays checked.
    await expect(page.getByTestId("remember-browser")).toBeChecked();

    await page.screenshot({ path: "e2e/__screenshots__/session-remembered.png", fullPage: true });

    // It is the SAME persisted session (same expiry), not a freshly minted one.
    expect(await page.getByTestId("session-status").textContent()).toBe(expiryText);
  });

  // The mock-setup harness exposes the session-manager cert + the FROZEN header
  // key on `window.__session` so we can prove, in a REAL browser, that a minted
  // session yields the attachable `concerto-device-cert` header. (The connect
  // interceptor's mutate-the-Headers behavior itself is unit-tested in
  // packages/client/src/session.test.ts.)
  test("a connected browser exposes the concerto-device-cert header value", async ({ page }) => {
    await page.goto("/?mock=1");
    await page.getByTestId("connect").click();
    await expect(page.getByTestId("session-status")).toHaveAttribute("data-session", "paired", {
      timeout: 10_000,
    });

    const result = await page.evaluate(() => window.__session!.headerForCurrentSession());
    expect(result.key).toBe("concerto-device-cert");
    expect(result.hasHeader).toBe(true);
    expect(result.deviceKind).toBe("web_ephemeral");
  });
});

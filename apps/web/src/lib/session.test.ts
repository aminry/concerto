import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createMemorySessionStore } from "@concerto/client";

import { SessionManager } from "./session";

// Regression coverage for the Task-522 review fix: getCert MUST stop attaching
// the `web_ephemeral` cert once it has expired (an 8h-stale tab must re-mint
// rather than keep presenting a dead cert), and clear-on-close MUST notify
// status listeners so the chip flips to "cleared".
describe("SessionManager session lifecycle (522 review fixes)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-06-16T00:00:00Z"));
  });
  afterEach(() => vi.useRealTimers());

  it("attaches the cert while valid, then returns null once expired (8h ttl)", async () => {
    const mgr = new SessionManager(createMemorySessionStore());
    await mgr.ensureSession(false);

    expect(mgr.getCert()).not.toBeNull();

    // Jump just past the 8h expiry — getCert must now refuse the stale cert.
    vi.setSystemTime(new Date("2026-06-16T08:00:01Z"));
    expect(mgr.getCert()).toBeNull();
  });

  it("clear() emits a 'cleared' status and drops the cert", async () => {
    const mgr = new SessionManager(createMemorySessionStore());
    const seen: string[] = [];
    mgr.onStatus((s) => seen.push(s.kind));

    await mgr.ensureSession(false);
    expect(mgr.getCert()).not.toBeNull();

    await mgr.clear();
    expect(mgr.getCert()).toBeNull();
    expect(seen).toContain("cleared");
  });
});

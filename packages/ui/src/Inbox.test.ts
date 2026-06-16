import { describe, expect, it } from "vitest";

import { NotificationKind } from "@concerto/client/gen/concerto/v1/notifications_pb";

import { kindLabel, relativeTime } from "./Inbox";

// Unit tests for the inbox's pure rendering helpers — the severity/kind/time
// logic shared by desktop + web. The component render itself is exercised by the
// Playwright UI-E2E suite (apps/web/e2e) against the real markup; here we pin the
// label map + the relative-time bucketing so a wire-enum change is caught early.

describe("kindLabel", () => {
  it("maps each known NotificationKind to its label", () => {
    expect(kindLabel(NotificationKind.TOOL_APPROVAL_NEEDED)).toBe("Approval needed");
    expect(kindLabel(NotificationKind.AGENT_COMPLETED_WITH_MESSAGE)).toBe("Agent completed");
    expect(kindLabel(NotificationKind.AGENT_CRASHED)).toBe("Agent crashed");
    expect(kindLabel(NotificationKind.PR_STATE_CHANGED)).toBe("PR updated");
    expect(kindLabel(NotificationKind.CHECK_RUN_FAILED)).toBe("Check failed");
    expect(kindLabel(NotificationKind.SCHEDULE_RUN_COMPLETED)).toBe("Schedule run");
  });

  it("falls back to 'Notification' for the unspecified kind", () => {
    expect(kindLabel(NotificationKind.UNSPECIFIED)).toBe("Notification");
  });
});

describe("relativeTime", () => {
  it("returns empty for a zero timestamp", () => {
    expect(relativeTime(0n)).toBe("");
  });

  it("buckets recent timestamps as 'just now'", () => {
    expect(relativeTime(BigInt(Date.now()))).toBe("just now");
  });

  it("buckets minutes / hours / days", () => {
    const now = Date.now();
    expect(relativeTime(BigInt(now - 5 * 60_000))).toBe("5m ago");
    expect(relativeTime(BigInt(now - 3 * 3_600_000))).toBe("3h ago");
    expect(relativeTime(BigInt(now - 2 * 86_400_000))).toBe("2d ago");
  });
});

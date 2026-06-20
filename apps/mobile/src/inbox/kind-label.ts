// Human-readable labels for each `NotificationKind` (Task 508). Wired to
// @concerto/client's generated proto enum — mobile consumes ONLY @concerto/client
// (PHASE5_PLANNING D11). Mirrors apps/web's KIND_LABEL so the two clients render
// the same notification taxonomy; the RN component tree itself is fresh.
import { NotificationKind } from "@concerto/client/gen/concerto/v1/notifications_pb";

export const KIND_LABEL: Record<NotificationKind, string> = {
  [NotificationKind.UNSPECIFIED]: "Notification",
  [NotificationKind.TOOL_APPROVAL_NEEDED]: "Approval needed",
  [NotificationKind.AGENT_COMPLETED_WITH_MESSAGE]: "Agent completed",
  [NotificationKind.AGENT_CRASHED]: "Agent crashed",
  [NotificationKind.PR_STATE_CHANGED]: "PR updated",
  [NotificationKind.CHECK_RUN_FAILED]: "Check failed",
  [NotificationKind.SCHEDULE_RUN_COMPLETED]: "Schedule run",
};

/** Label for a kind, falling back to the generic "Notification". */
export function kindLabel(kind: NotificationKind): string {
  return KIND_LABEL[kind] ?? "Notification";
}

/** Compact relative time for a unix-ms timestamp (proto carries `int64` ⇒ `bigint`). */
export function relativeTime(ms: bigint): string {
  const then = Number(ms);
  if (!then) return "";
  const min = Math.round((Date.now() - then) / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.round(hr / 24)}d ago`;
}

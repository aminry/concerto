//! The shared notifications inbox renderer (Task 523, decision D11).
//
// Extracted from `apps/web/src/App.tsx` so desktop + web render the SAME inbox
// (`InboxView` + `NotificationCard` + the severity/kind/time rendering). The
// component is transport-agnostic: the host owns the connection (the web app's
// connect bar; the desktop shell's Tauri/iroh transport) and passes the already
// fetched notification list + the handlers + the load-state flags as props. The
// styling is portable via the co-located `inbox.css` the consumer imports
// (`import "@concerto/ui/inbox.css"`); web inherits it as-is, desktop wraps it.

import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { NotificationKind } from "@concerto/client/gen/concerto/v1/notifications_pb";

/** Human label per `NotificationKind` (kept beside the renderer it drives). */
const KIND_LABEL: Record<NotificationKind, string> = {
  [NotificationKind.UNSPECIFIED]: "Notification",
  [NotificationKind.TOOL_APPROVAL_NEEDED]: "Approval needed",
  [NotificationKind.AGENT_COMPLETED_WITH_MESSAGE]: "Agent completed",
  [NotificationKind.AGENT_CRASHED]: "Agent crashed",
  [NotificationKind.PR_STATE_CHANGED]: "PR updated",
  [NotificationKind.CHECK_RUN_FAILED]: "Check failed",
  [NotificationKind.SCHEDULE_RUN_COMPLETED]: "Schedule run",
};

/** Load state for the inbox feed, owned by the host's data layer. */
export type InboxStatus =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ok"; count: number }
  | { kind: "error"; message: string };

/** Props for [`Inbox`]. The host supplies the data + handlers + the load state. */
export interface InboxProps {
  /** The notification feed (newest-first), as fetched by the host. */
  items: Notification[];
  /** Current load state — drives the idle / loading / empty / error surfaces. */
  status: InboxStatus;
  /** Whether the "unread only" filter is on (controlled by the host). */
  unreadOnly: boolean;
  /** Toggle the "unread only" filter; the host refetches with the new value. */
  onUnreadOnlyChange: (value: boolean) => void;
  /** Mark a single notification read by id; the host calls the service + refetches. */
  onMarkRead: (id: string) => void;
}

/** Format a created-at epoch-ms as a coarse relative time ("3m ago", "2d ago"). */
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

/** Map a `NotificationKind` to its display label (falls back to "Notification"). */
export function kindLabel(kind: NotificationKind): string {
  return KIND_LABEL[kind] ?? "Notification";
}

/** The defined severity buckets the inbox styles + renders. */
export type Severity = "low" | "medium" | "high";

/**
 * Normalize a free-form wire `severity` string (`notifications.proto`'s
 * `string severity`) to a known bucket. The Core promises `"low" | "medium" |
 * "high"`, but it is an open wire string — an unexpected value (`"critical"`,
 * `"info"`, whitespace, empty) would otherwise yield an unstyled accent + pill
 * AND inject the raw string as the tag text. Anything outside the known set
 * collapses to `"low"`, so the card always renders a defined bucket.
 */
export function severityBucket(severity: string): Severity {
  return severity === "low" || severity === "medium" || severity === "high" ? severity : "low";
}

/** One severity-coded notification card with its mark-read affordance. */
export function NotificationCard({
  notification: n,
  onMarkRead,
}: {
  notification: Notification;
  onMarkRead: (id: string) => void;
}) {
  // Normalize the free-form wire severity to a defined bucket so an unexpected
  // value (e.g. "critical") still styles + renders as a known severity.
  const sev = severityBucket(n.severity);
  return (
    <li
      className={`card sev-${sev}${n.readAtMs ? " read" : ""}`}
      data-testid="notification"
    >
      <span className="accent" aria-hidden="true" />
      <div className="card-body">
        <div className="card-head">
          <span className="kind">{kindLabel(n.kind)}</span>
          <span className="dot" aria-hidden="true">
            ·
          </span>
          <span className={`sev-tag ${sev}`}>{sev}</span>
          <span className="spacer" />
          <time className="time">{relativeTime(n.createdAtMs)}</time>
        </div>
        <p className="card-title">{n.title}</p>
        {n.body && <p className="card-text">{n.body}</p>}
      </div>
      {!n.readAtMs && (
        <button className="btn ghost" onClick={() => onMarkRead(n.id)} data-testid="mark-read">
          Mark read
        </button>
      )}
    </li>
  );
}

/**
 * The notifications inbox: the title + unread-only toggle, the idle / empty /
 * error surfaces, and the severity-coded feed. Purely presentational — every
 * piece of state (the feed, the load status, the filter) is owned by the host,
 * so the same component renders against the web connect-web transport and the
 * desktop Tauri/iroh transport alike.
 */
export function Inbox({ items, status, unreadOnly, onUnreadOnlyChange, onMarkRead }: InboxProps) {
  return (
    <div className="inbox" data-testid="inbox">
      <div className="toolbar">
        <h1 className="title">Notifications</h1>
        <label className="toggle">
          <input
            type="checkbox"
            checked={unreadOnly}
            onChange={(e) => onUnreadOnlyChange(e.target.checked)}
            data-testid="unread-toggle"
          />
          <span>Unread only</span>
        </label>
      </div>

      {status.kind === "error" && (
        <div className="banner error" role="alert" data-testid="error">
          Couldn’t load the inbox — {status.message}
        </div>
      )}

      {status.kind === "loading" && items.length === 0 && (
        <div className="empty" role="status" aria-live="polite" data-testid="loading">
          <p className="empty-sub">Loading…</p>
        </div>
      )}

      {status.kind === "idle" && (
        <div className="empty" role="status" aria-live="polite" data-testid="idle">
          <p className="empty-title">Connect to a Core</p>
          <p className="empty-sub">Connect to your Core to load the inbox.</p>
        </div>
      )}

      {status.kind === "ok" && items.length === 0 && (
        <div className="empty" role="status" aria-live="polite" data-testid="empty">
          <p className="empty-title">You’re all caught up</p>
          <p className="empty-sub">No {unreadOnly ? "unread " : ""}notifications.</p>
        </div>
      )}

      {items.length > 0 && (
        <ul className="feed" data-testid="feed">
          {items.map((n) => (
            <NotificationCard key={n.id} notification={n} onMarkRead={onMarkRead} />
          ))}
        </ul>
      )}
    </div>
  );
}

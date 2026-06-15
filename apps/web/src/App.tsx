import { useCallback, useState } from "react";

import type { DataClient } from "@concerto/client";
import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { NotificationKind } from "@concerto/client/gen/concerto/v1/notifications_pb";

import { fetchInbox, makeDataClient, markRead } from "./lib/data";

// The Core's connect-web bridge (CONCERTO_CONNECT_BRIDGE) default loopback port.
const DEFAULT_BASE_URL = "http://127.0.0.1:8787";

const KIND_LABEL: Record<NotificationKind, string> = {
  [NotificationKind.UNSPECIFIED]: "Notification",
  [NotificationKind.TOOL_APPROVAL_NEEDED]: "Approval needed",
  [NotificationKind.AGENT_COMPLETED_WITH_MESSAGE]: "Agent completed",
  [NotificationKind.AGENT_CRASHED]: "Agent crashed",
  [NotificationKind.PR_STATE_CHANGED]: "PR updated",
  [NotificationKind.CHECK_RUN_FAILED]: "Check failed",
  [NotificationKind.SCHEDULE_RUN_COMPLETED]: "Schedule run",
};

type Status =
  | { kind: "idle" }
  | { kind: "loading" }
  | { kind: "ok"; count: number }
  | { kind: "error"; message: string };

function relativeTime(ms: bigint): string {
  const then = Number(ms);
  if (!then) return "";
  const min = Math.round((Date.now() - then) / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const hr = Math.round(min / 60);
  if (hr < 24) return `${hr}h ago`;
  return `${Math.round(hr / 24)}d ago`;
}

export function App() {
  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [items, setItems] = useState<Notification[]>([]);

  const refresh = useCallback(async () => {
    setStatus({ kind: "loading" });
    try {
      const dc: DataClient = makeDataClient(baseUrl);
      const notifs = await fetchInbox(dc, { unreadOnly });
      setItems(notifs);
      setStatus({ kind: "ok", count: notifs.length });
    } catch (e) {
      setStatus({ kind: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, [baseUrl, unreadOnly]);

  const onMarkRead = useCallback(
    async (id: string) => {
      try {
        await markRead(makeDataClient(baseUrl), id);
        await refresh();
      } catch {
        // Surfaced on the next refresh.
      }
    },
    [baseUrl, refresh],
  );

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true" />
          <span className="brand-name">Concerto</span>
          <span className="brand-sub">Inbox</span>
        </div>
        <form
          className="connect"
          onSubmit={(e) => {
            e.preventDefault();
            void refresh();
          }}
        >
          <input
            className="url"
            aria-label="Core address"
            value={baseUrl}
            onChange={(e) => setBaseUrl(e.target.value)}
            placeholder="http://127.0.0.1:8787"
            data-testid="core-url"
          />
          <button className="btn primary" type="submit" data-testid="connect">
            {status.kind === "loading" ? "Loading…" : "Connect"}
          </button>
        </form>
      </header>

      <main className="content">
        <div className="toolbar">
          <h1 className="title">Notifications</h1>
          <label className="toggle">
            <input
              type="checkbox"
              checked={unreadOnly}
              onChange={(e) => {
                setUnreadOnly(e.target.checked);
              }}
              data-testid="unread-toggle"
            />
            <span>Unread only</span>
          </label>
        </div>

        {status.kind === "error" && (
          <div className="banner error" role="alert" data-testid="error">
            Couldn’t reach the Core at <code>{baseUrl}</code> — {status.message}
          </div>
        )}

        {status.kind === "idle" && (
          <div className="empty" data-testid="idle">
            <p className="empty-title">Connect to a Core</p>
            <p className="empty-sub">
              Enter your Core’s address and press Connect to load the inbox.
            </p>
          </div>
        )}

        {status.kind === "ok" && items.length === 0 && (
          <div className="empty" data-testid="empty">
            <p className="empty-title">You’re all caught up</p>
            <p className="empty-sub">No {unreadOnly ? "unread " : ""}notifications.</p>
          </div>
        )}

        {items.length > 0 && (
          <ul className="feed" data-testid="feed">
            {items.map((n) => (
              <li
                key={n.id}
                className={`card sev-${n.severity || "low"}${n.readAtMs ? " read" : ""}`}
                data-testid="notification"
              >
                <span className="accent" aria-hidden="true" />
                <div className="card-body">
                  <div className="card-head">
                    <span className="kind">{KIND_LABEL[n.kind] ?? "Notification"}</span>
                    <span className="dot" aria-hidden="true">
                      ·
                    </span>
                    <span className={`sev-tag ${n.severity || "low"}`}>{n.severity || "low"}</span>
                    <span className="spacer" />
                    <time className="time">{relativeTime(n.createdAtMs)}</time>
                  </div>
                  <p className="card-title">{n.title}</p>
                  {n.body && <p className="card-text">{n.body}</p>}
                </div>
                {!n.readAtMs && (
                  <button
                    className="btn ghost"
                    onClick={() => void onMarkRead(n.id)}
                    data-testid="mark-read"
                  >
                    Mark read
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </main>
    </div>
  );
}

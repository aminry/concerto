import { useCallback, useState } from "react";

import type { DataClient } from "@concerto/client";
import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Inbox, type InboxStatus } from "@concerto/ui";

import { fetchInbox, makeDataClient, markRead } from "./lib/data";

// The Core's connect-web bridge (CONCERTO_CONNECT_BRIDGE) default loopback port.
const DEFAULT_BASE_URL = "http://127.0.0.1:8787";

export function App() {
  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [status, setStatus] = useState<InboxStatus>({ kind: "idle" });
  const [items, setItems] = useState<Notification[]>([]);

  const refresh = useCallback(
    async (next: { unreadOnly?: boolean } = {}) => {
      const filterUnread = next.unreadOnly ?? unreadOnly;
      setStatus({ kind: "loading" });
      try {
        const dc: DataClient = makeDataClient(baseUrl);
        const notifs = await fetchInbox(dc, { unreadOnly: filterUnread });
        setItems(notifs);
        setStatus({ kind: "ok", count: notifs.length });
      } catch (e) {
        const detail = e instanceof Error ? e.message : String(e);
        setStatus({ kind: "error", message: `couldn’t reach the Core at ${baseUrl} — ${detail}` });
      }
    },
    [baseUrl, unreadOnly],
  );

  const onUnreadOnlyChange = useCallback(
    (value: boolean) => {
      setUnreadOnly(value);
      // Only refetch once connected; the idle state stays put until Connect.
      if (status.kind !== "idle") void refresh({ unreadOnly: value });
    },
    [refresh, status.kind],
  );

  const onMarkRead = useCallback(
    (id: string) => {
      void (async () => {
        try {
          await markRead(makeDataClient(baseUrl), id);
          await refresh();
        } catch {
          // Surfaced on the next refresh.
        }
      })();
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
        {/* The shared, transport-agnostic inbox (@concerto/ui). The web shell
            owns the connection (the connect bar above) + the data fetch; the
            component renders the feed + filter + idle/empty/error surfaces. */}
        <Inbox
          items={items}
          status={status}
          unreadOnly={unreadOnly}
          onUnreadOnlyChange={onUnreadOnlyChange}
          onMarkRead={onMarkRead}
        />
      </main>
    </div>
  );
}

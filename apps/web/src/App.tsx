import { useCallback, useEffect, useRef, useState } from "react";

import type { DataClient, Unsubscribe } from "@concerto/client";
import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Inbox, type InboxStatus } from "@concerto/ui";

import { fetchInbox, type LiveStatus, makeDataClient, markRead, subscribeLiveInbox } from "./lib/data";
import { sessionManager, type SessionStatus } from "./lib/session";

// The Core's connect-web bridge (CONCERTO_CONNECT_BRIDGE) default loopback port.
const DEFAULT_BASE_URL = "http://127.0.0.1:8787";

/** "remember browser" preference persists outside IndexedDB so it survives a clear. */
const REMEMBER_KEY = "concerto.web.remember";

function loadRememberPref(): boolean {
  try {
    return globalThis.localStorage?.getItem(REMEMBER_KEY) === "1";
  } catch {
    return false;
  }
}

function saveRememberPref(value: boolean): void {
  try {
    globalThis.localStorage?.setItem(REMEMBER_KEY, value ? "1" : "0");
  } catch {
    // localStorage unavailable (private mode / SSR) — the IndexedDB flag still holds.
  }
}

/** Human "expires in …" from an epoch-ms expiry. */
function expiresIn(expiresAt: number, nowMs: number = Date.now()): string {
  const ms = expiresAt - nowMs;
  if (ms <= 0) return "expired";
  const mins = Math.round(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  const rem = mins % 60;
  return rem === 0 ? `${hrs}h` : `${hrs}h ${rem}m`;
}

/** The session status chip + "remember browser" opt-out (shell chrome, Task 522). */
function SessionControls({
  session,
  remember,
  onRememberChange,
}: {
  session: SessionStatus;
  remember: boolean;
  onRememberChange: (value: boolean) => void;
}) {
  return (
    <div className="session" data-testid="session">
      <span
        className={`session-badge ${session.kind}`}
        data-testid="session-status"
        data-session={session.kind}
        title={
          session.kind === "paired"
            ? "This browser holds an 8h ephemeral session cert (web_ephemeral)"
            : session.kind === "cleared"
              ? "Session cleared — reconnect to mint a new one"
              : "No session yet — Connect to pair this browser"
        }
      >
        <span className="session-dot" aria-hidden="true" />
        {session.kind === "paired"
          ? `Paired · expires in ${expiresIn(session.expiresAt)}`
          : session.kind === "cleared"
            ? "Cleared"
            : "Not paired"}
      </span>
      <label className="remember">
        <input
          type="checkbox"
          checked={remember}
          onChange={(e) => onRememberChange(e.target.checked)}
          data-testid="remember-browser"
        />
        Remember browser
      </label>
    </div>
  );
}

/** Merge fresh (newest-first) notifications into the head of the feed, deduped by id. */
function prepend(existing: Notification[], fresh: Notification[]): Notification[] {
  if (fresh.length === 0) return existing;
  const incoming = new Set(fresh.map((n) => n.id));
  const kept = existing.filter((n) => !incoming.has(n.id));
  return [...fresh, ...kept];
}

export function App() {
  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL);
  const [unreadOnly, setUnreadOnly] = useState(false);
  const [status, setStatus] = useState<InboxStatus>({ kind: "idle" });
  const [items, setItems] = useState<Notification[]>([]);
  // Live-updates transport mode (Task 520); null until a subscription is up.
  const [live, setLive] = useState<LiveStatus | null>(null);
  // Ephemeral browser session (Task 522): status chip + remember-browser opt-out.
  const [session, setSession] = useState<SessionStatus>({ kind: "none" });
  const [remember, setRemember] = useState(loadRememberPref);

  // Subscribe to session-manager status + restore a remembered session on boot.
  useEffect(() => {
    const unsub = sessionManager.onStatus(setSession);
    void sessionManager.restore();
    return unsub;
  }, []);

  const onRememberChange = useCallback(
    (value: boolean) => {
      setRemember(value);
      saveRememberPref(value);
      // If a session is already live, re-apply the preference (re-persist the
      // remember flag + arm/disarm clear-on-close). Pre-Connect this only stores
      // the preference; the session is minted on Connect.
      if (session.kind === "paired") void sessionManager.ensureSession(value);
    },
    [session.kind],
  );

  // Hold the active live subscription so it survives re-renders and is torn down
  // on reconnect / filter change / unmount.
  const liveUnsub = useRef<Unsubscribe | null>(null);
  const stopLive = useCallback(() => {
    liveUnsub.current?.();
    liveUnsub.current = null;
    setLive(null);
  }, []);

  const refresh = useCallback(
    async (next: { unreadOnly?: boolean } = {}) => {
      const filterUnread = next.unreadOnly ?? unreadOnly;
      setStatus({ kind: "loading" });
      // Drop any prior live subscription before reconnecting (Task 520).
      stopLive();
      try {
        // Task 522: pair this browser — mint (or reuse a valid) 8h `web_ephemeral`
        // session cert via the stub-phone signer + store it BEFORE the first call,
        // so the connect interceptor can attach the `concerto-device-cert` header.
        await sessionManager.ensureSession(remember);
        const dc: DataClient = makeDataClient(baseUrl);
        const notifs = await fetchInbox(dc, { unreadOnly: filterUnread });
        setItems(notifs);
        setStatus({ kind: "ok", count: notifs.length });
        // Start live updates: stream new notifications, fall back to polling.
        liveUnsub.current = subscribeLiveInbox(dc, {
          unreadOnly: filterUnread,
          onNotifications: (fresh) =>
            setItems((prev) => {
              const merged = prepend(prev, fresh);
              setStatus({ kind: "ok", count: merged.length });
              return merged;
            }),
          onStatus: setLive,
        });
      } catch (e) {
        const detail = e instanceof Error ? e.message : String(e);
        setStatus({ kind: "error", message: `couldn’t reach the Core at ${baseUrl} — ${detail}` });
      }
    },
    [baseUrl, unreadOnly, stopLive, remember],
  );

  // Tear the live subscription down when the shell unmounts.
  useEffect(() => stopLive, [stopLive]);

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
          {/* Subtle live-updates indicator (Task 520) — shell chrome, NOT the
              inbox. Only shown once a subscription is active. */}
          {live !== null && (
            <span
              className={`live-badge ${live}`}
              data-testid="live-status"
              data-live={live}
              title={
                live === "live"
                  ? "Live updates via notification.events stream"
                  : "Live updates via polling fallback"
              }
            >
              <span className="live-dot" aria-hidden="true" />
              {live === "live" ? "Live" : "Polling"}
            </span>
          )}
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
        {/* Task 522: ephemeral browser pairing — session status + remember opt-out.
            Shell chrome only; the @concerto/ui inbox is untouched. */}
        <SessionControls session={session} remember={remember} onRememberChange={onRememberChange} />
      </header>

      <main className="content">
        {/* The shared, transport-agnostic inbox (@concerto/ui). The web shell
            owns the connection (the connect bar above) + the data fetch + the
            live subscription (Task 520); the component renders the feed + filter
            + idle/empty/error surfaces and is NOT restyled here. */}
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

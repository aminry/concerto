//! Web data layer (Task 519/520): builds the connect-web `DataClient` and the
//! typed Notifications client off `@concerto/client`, plus the live-updates
//! subscription (520 — `notification.events` stream with an AckOffset polling
//! fallback). Auth/TLS/pairing are layered on by Tasks 521–522.

import {
  createClient,
  createConnectWebDataClient,
  type DataClient,
  type LiveStatus,
  subscribeNotificationsLive,
  type Unsubscribe,
} from "@concerto/client";
import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Notifications } from "@concerto/client/gen/concerto/v1/notifications_pb";

export type { LiveStatus } from "@concerto/client";

/**
 * Test-only injection seam (Task 520 mock e2e): when the harness installs a
 * `DataClient` on `window.__CONCERTO_TEST_DATA_CLIENT__`, the app uses it instead
 * of connect-web — so the Playwright mock spec drives live updates with no real
 * Core. Production never sets this; the connect-web path is unchanged.
 */
declare global {
  interface Window {
    __CONCERTO_TEST_DATA_CLIENT__?: DataClient;
  }
}

/** Build a web data client against the Core's connect-web bridge at `baseUrl`. */
export function makeDataClient(baseUrl: string): DataClient {
  if (typeof window !== "undefined" && window.__CONCERTO_TEST_DATA_CLIENT__) {
    return window.__CONCERTO_TEST_DATA_CLIENT__;
  }
  return createConnectWebDataClient({ baseUrl });
}

/** Fetch the inbox feed (newest-first) over the live Notifications service. */
export async function fetchInbox(
  dc: DataClient,
  opts: { unreadOnly?: boolean } = {},
): Promise<Notification[]> {
  const client = createClient(Notifications, dc.transport);
  const res = await client.getInbox({ unreadOnly: opts.unreadOnly ?? false, limit: 0 });
  return res.notifications;
}

/** Mark a notification read over the live service. */
export async function markRead(dc: DataClient, id: string): Promise<void> {
  const client = createClient(Notifications, dc.transport);
  await client.markRead({ id });
}

/** Callbacks for [`subscribeLiveInbox`]. */
export interface LiveInboxHandlers {
  /** New notifications arrived (deduped by id); the shell prepends them. */
  onNotifications: (fresh: Notification[]) => void;
  /** The transport mode flipped between the stream and the poll fallback. */
  onStatus: (status: LiveStatus) => void;
}

/**
 * Subscribe to live notifications over the `notification.events` stream with an
 * AckOffset polling fallback (Task 520). Newly arrived notifications (deduped by
 * id across both paths) flow to `onNotifications`; the `"live" | "polling"`
 * transport mode flows to `onStatus`. Returns an unsubscribe.
 */
export function subscribeLiveInbox(
  dc: DataClient,
  opts: { unreadOnly?: boolean } & LiveInboxHandlers,
): Unsubscribe {
  return subscribeNotificationsLive(dc, {
    unreadOnly: opts.unreadOnly ?? false,
    onNotifications: opts.onNotifications,
    onStatus: opts.onStatus,
  });
}

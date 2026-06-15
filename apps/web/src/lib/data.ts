//! Web data layer (Task 519): builds the connect-web `DataClient` and the typed
//! Notifications client off `@concerto/client`. Auth/TLS/pairing + the SSE
//! fallback are layered on by Tasks 520–522.

import { createClient, createConnectWebDataClient, type DataClient } from "@concerto/client";
import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Notifications } from "@concerto/client/gen/concerto/v1/notifications_pb";

/** Build a web data client against the Core's connect-web bridge at `baseUrl`. */
export function makeDataClient(baseUrl: string): DataClient {
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

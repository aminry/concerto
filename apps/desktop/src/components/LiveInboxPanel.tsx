// Live desktop inbox (Task 523 follow-up).
//
// Wires the shared `@concerto/ui` `Inbox` to the Core's live `Notifications`
// service over the `concerto_rpc` bridge: fetches `GetInbox` on mount, refetches
// on every `notification.events` frame (so a notification created / read / acted
// anywhere — incl. the web client — reflects here live, the design/14 R-8
// cross-device sync), and drives `MarkRead`. Replaces the static `InboxPanel`
// idle mount, so desktop now shows the same live inbox as the web client.

import { useCallback, useEffect, useRef, useState } from "react";

import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Inbox, type InboxStatus } from "@concerto/ui";

// The shared inbox styles (same import the web client + the old InboxPanel use).
import "@concerto/ui/inbox.css";

import { onConcertoEvent, subscribe, unsubscribe } from "../api/client";
import { getInbox, markRead } from "../api/notifications";

const SUBJECT = "notification.events";

export function LiveInboxPanel(): JSX.Element {
  const [items, setItems] = useState<Notification[]>([]);
  const [status, setStatus] = useState<InboxStatus>({ kind: "loading" });
  const [unreadOnly, setUnreadOnly] = useState(false);
  const unreadRef = useRef(unreadOnly);
  unreadRef.current = unreadOnly;

  const refresh = useCallback(async (unread = unreadRef.current) => {
    try {
      const notifs = await getInbox(unread);
      setItems(notifs);
      setStatus({ kind: "ok", count: notifs.length });
    } catch (e) {
      setStatus({ kind: "error", message: e instanceof Error ? e.message : String(e) });
    }
  }, []);

  // Initial fetch + a live subscription to `notification.events`: refetch the
  // (cheap, always-available) inbox read on any frame.
  useEffect(() => {
    let subId: string | undefined;
    let unlisten: (() => void) | undefined;
    void refresh();
    void (async () => {
      try {
        unlisten = await onConcertoEvent(SUBJECT, () => void refresh());
        subId = await subscribe(SUBJECT);
      } catch {
        // No stream available — the inbox still works via the initial fetch and
        // mark-read refetch; live updates just won't push until reconnect.
      }
    })();
    return () => {
      unlisten?.();
      if (subId) void unsubscribe(subId);
    };
  }, [refresh]);

  const onUnreadOnlyChange = useCallback(
    (value: boolean) => {
      setUnreadOnly(value);
      void refresh(value);
    },
    [refresh],
  );

  const onMarkRead = useCallback(
    (id: string) => {
      void (async () => {
        try {
          await markRead(id);
          await refresh();
        } catch {
          // Surfaced on the next refresh.
        }
      })();
    },
    [refresh],
  );

  return (
    <div className="h-full overflow-y-auto p-4" data-testid="desktop-inbox">
      <Inbox
        items={items}
        status={status}
        unreadOnly={unreadOnly}
        onUnreadOnlyChange={onUnreadOnlyChange}
        onMarkRead={onMarkRead}
      />
    </div>
  );
}

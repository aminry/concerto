// Desktop → Core Notifications binding (Task 523 follow-up).
//
// Reads the LIVE `Notifications` service over the `concerto_rpc` bridge and maps
// the Core's serde-JSON (snake_case, ms-int timestamps, enum-as-int `kind`) to
// the `@concerto/ui` `Notification` shape (the camelCase / bigint fields the
// shared `Inbox` card renders). This is the wiring 523 deferred — desktop now
// shows the same live inbox as the web client.

import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { NotificationKind } from "@concerto/client/gen/concerto/v1/notifications_pb";

import { callRpc } from "./client";

/** A notification row exactly as the Core's serde-JSON returns it (snake_case).
    `kind` arrives as the full proto enum NAME string (e.g.
    `"NOTIFICATION_KIND_AGENT_CRASHED"`), and the ms timestamps may be a number
    or a numeric string — `BigInt(...)` accepts both. */
interface RawNotification {
  id: string;
  kind: number | string;
  severity?: string;
  title?: string;
  body?: string;
  created_at_ms?: number | string;
  read_at_ms?: number | string | null;
}

/** Map the Core's wire `kind` to the protobuf-es `NotificationKind` number the
    shared `kindLabel` keys on. serde emits the full proto name
    (`NOTIFICATION_KIND_AGENT_CRASHED`); protobuf-es strips the prefix
    (`AGENT_CRASHED = 3`), so strip-then-look-up the enum. */
function kindFromWire(kind: number | string): NotificationKind {
  if (typeof kind === "number") return kind as NotificationKind;
  const name = kind.replace(/^NOTIFICATION_KIND_/, "");
  const value = (NotificationKind as unknown as Record<string, number>)[name];
  return (typeof value === "number" ? value : NotificationKind.UNSPECIFIED) as NotificationKind;
}

/** Map a Core JSON row to the fields the shared `@concerto/ui` Inbox reads. */
function toUiNotification(raw: RawNotification): Notification {
  return {
    id: raw.id,
    kind: kindFromWire(raw.kind),
    severity: raw.severity ?? "low",
    title: raw.title ?? "",
    body: raw.body ?? "",
    createdAtMs: BigInt(raw.created_at_ms ?? 0),
    readAtMs: raw.read_at_ms ? BigInt(raw.read_at_ms) : undefined,
    // The card reads only the fields above; the rest of the proto message is
    // not needed by the shared renderer.
  } as unknown as Notification;
}

/** Fetch the inbox feed (newest-first), optionally unread-only. */
export async function getInbox(unreadOnly: boolean): Promise<Notification[]> {
  const res = await callRpc<
    { unread_only: boolean; limit: number },
    { notifications?: RawNotification[] }
  >("Notifications.GetInbox", { unread_only: unreadOnly, limit: 0 });
  return (res.notifications ?? []).map(toUiNotification);
}

/** Mark a notification read by id. */
export async function markRead(id: string): Promise<void> {
  await callRpc<{ id: string }, unknown>("Notifications.MarkRead", { id });
}

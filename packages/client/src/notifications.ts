//! Live-notifications layer over the transport-agnostic [`DataClient`] seam
//! (Task 520, design/14 §5.3). Two pieces, both transport-agnostic — they take a
//! [`DataClient`] (web/desktop/native all qualify) and never reference
//! connect-web:
//!
//!   1. `subscribeNotifications` — consumes the `notification.events` server
//!      stream (via `DataClient.subscribe`), decodes each frame's opaque
//!      `Event.checks_opaque` JSON payload into a typed [`NotificationFrame`],
//!      and refetches the affected notification(s) so callers get a live
//!      `Notification` to prepend.
//!   2. `pollInbox` — the AckOffset-based polling FALLBACK: re-calls `GetInbox`
//!      from a cursor on an interval, dedups by id, advances an offset cursor,
//!      and (best-effort) `AckOffset`s the `notification.events` subject so the
//!      Core can prune its ring buffer. Used when streaming is unavailable or
//!      errors (design/10 §3.2 R-2 — the Connect-Web fallback path).
//!
//! `subscribeNotificationsLive` ties them together: stream first, fall back to
//! polling on a stream error, and surface a `"live" | "polling"` status.

import { createClient } from "@connectrpc/connect";

import type { DataClient, Unsubscribe } from "./data-client";
import type { Notification } from "./gen/concerto/v1/notifications_pb";
import { Notifications } from "./gen/concerto/v1/notifications_pb";
import type { Event } from "./gen/concerto/v1/streams_pb";
import { Streams } from "./gen/concerto/v1/streams_pb";

/** The `notification.events` streams subject (unscoped; design/14 §5.3). */
export const NOTIFICATION_EVENTS_SUBJECT = "notification.events";

/**
 * A decoded `notification.events` frame — the opaque JSON carried on
 * `Event.checks_opaque` (design/14 §5.3; FROZEN by the Core's `events::to_frame`).
 * `kind` is the bare lifecycle verb (the Core sends `"notification.created"` etc.;
 * we strip the `notification.` prefix so callers switch on `"created"`).
 */
export interface NotificationFrame {
  /** The lifecycle verb: created / updated / read / acted. */
  kind: "created" | "updated" | "read" | "acted" | string;
  /** The affected notification id. */
  id: string;
  /** The chip's `rule_id` — present only on `"acted"`. */
  chipId?: string;
  /** The acting device id — present only on `"acted"`. */
  byDeviceId?: string;
  /** The stream offset of the carrying `Event` (drives AckOffset). */
  offset: bigint;
}

const textDecoder = new TextDecoder();

/**
 * Decode a `notification.events` [`Event`] into a [`NotificationFrame`], or
 * `null` if the event carries no parseable opaque frame (defensive — a future
 * Core could publish a body-only event on the subject). Tolerant of both the
 * prefixed (`"notification.created"`) and bare (`"created"`) `kind` spellings.
 */
export function parseNotificationFrame(ev: Event): NotificationFrame | null {
  const bytes = ev.checksOpaque;
  if (!bytes || bytes.length === 0) return null;
  let obj: unknown;
  try {
    obj = JSON.parse(textDecoder.decode(bytes));
  } catch {
    return null;
  }
  if (typeof obj !== "object" || obj === null) return null;
  const rec = obj as Record<string, unknown>;
  const rawKind = typeof rec.kind === "string" ? rec.kind : "";
  const id = typeof rec.id === "string" ? rec.id : "";
  if (!id) return null;
  const kind = rawKind.startsWith("notification.")
    ? rawKind.slice("notification.".length)
    : rawKind;
  return {
    kind,
    id,
    ...(typeof rec.chip_id === "string" ? { chipId: rec.chip_id } : {}),
    ...(typeof rec.by_device_id === "string" ? { byDeviceId: rec.by_device_id } : {}),
    offset: ev.offset,
  };
}

/** Options for [`subscribeNotifications`]. */
export interface SubscribeNotificationsOptions {
  /**
   * Fires per decoded frame. The frame's `kind`/`id` let the caller decide
   * whether to refetch (created/updated) or update local state (read/acted).
   */
  onFrame: (frame: NotificationFrame) => void;
  /** Fires if the underlying stream errors (after the first frame). */
  onError?: (err: unknown) => void;
}

/**
 * Subscribe to the `notification.events` server stream over a [`DataClient`],
 * decoding each frame. Returns an unsubscribe that aborts the stream. Pure
 * stream plumbing — the polling fallback is [`pollInbox`]; the orchestrator is
 * [`subscribeNotificationsLive`].
 */
export function subscribeNotifications(
  dc: DataClient,
  opts: SubscribeNotificationsOptions,
): Unsubscribe {
  return dc.subscribe(
    NOTIFICATION_EVENTS_SUBJECT,
    (ev) => {
      const frame = parseNotificationFrame(ev);
      if (frame) opts.onFrame(frame);
    },
    opts.onError,
  );
}

/** Options for [`pollInbox`]. */
export interface PollInboxOptions {
  /** Poll period in ms (default 5000). */
  intervalMs?: number;
  /** Restrict the inbox feed to unread notifications. */
  unreadOnly?: boolean;
  /**
   * Fires with notifications NOT seen before (deduped by id, newest-first as
   * returned by `GetInbox`). On the first tick this is the whole feed; on later
   * ticks only the newly arrived rows.
   */
  onNotifications: (fresh: Notification[]) => void;
  /** Fires on a `GetInbox` error (the poller keeps ticking). */
  onError?: (err: unknown) => void;
  /**
   * `setInterval`/`clearInterval` overrides (tests inject fakes). Defaults to
   * the global timers.
   */
  setInterval?: (cb: () => void, ms: number) => ReturnType<typeof globalThis.setInterval>;
  clearInterval?: (handle: ReturnType<typeof globalThis.setInterval>) => void;
  /** Fire the first poll immediately rather than after `intervalMs` (default true). */
  immediate?: boolean;
}

/** Handle returned by [`pollInbox`]: stop the poller + read the AckOffset cursor. */
export interface InboxPoller {
  /** Stop polling. Idempotent. */
  stop: () => void;
  /**
   * The highest stream offset acked so far on `notification.events`. Advances
   * when [`subscribeNotificationsLive`] threads stream offsets through; on a
   * pure-poll path it stays at 0 (no stream offsets observed). Exposed for the
   * AckOffset fallback + tests.
   */
  readonly ackOffset: bigint;
}

/**
 * AckOffset-based polling FALLBACK: re-call `GetInbox` from a cursor on an
 * interval, deduping by id so callers only ever see notifications they have not
 * seen. This is the Connect-Web fallback for live updates when the
 * `notification.events` stream is unavailable or errors (design/10 §3.2 R-2).
 *
 * Each tick also (best-effort) `AckOffset`s the `notification.events` subject at
 * the current offset cursor so the Core prunes its ring buffer — the offset is
 * advanced by [`subscribeNotificationsLive`] from observed stream frames before
 * a stream error drops us here (a pure-poll start acks 0, a no-op).
 */
export function pollInbox(dc: DataClient, opts: PollInboxOptions): InboxPoller {
  const intervalMs = opts.intervalMs ?? 5000;
  const setIv = opts.setInterval ?? ((cb, ms) => globalThis.setInterval(cb, ms));
  const clearIv = opts.clearInterval ?? ((h) => globalThis.clearInterval(h));
  const immediate = opts.immediate ?? true;
  const notifications = createClient(Notifications, dc.transport);
  const streams = createClient(Streams, dc.transport);

  const seen = new Set<string>();
  let ackOffset = 0n;
  let stopped = false;
  let handle: ReturnType<typeof globalThis.setInterval> | undefined;

  const tick = async () => {
    try {
      const res = await notifications.getInbox({
        unreadOnly: opts.unreadOnly ?? false,
        limit: 0,
      });
      if (stopped) return;
      const fresh = res.notifications.filter((n) => !seen.has(n.id));
      for (const n of fresh) seen.add(n.id);
      // Best-effort ack so the Core prunes the ring buffer; a 0 ack is a no-op.
      if (ackOffset > 0n) {
        void streams
          .ackOffset({ subject: NOTIFICATION_EVENTS_SUBJECT, offset: ackOffset })
          .catch(() => {});
      }
      if (fresh.length > 0) opts.onNotifications(fresh);
    } catch (err) {
      if (!stopped) opts.onError?.(err);
    }
  };

  if (immediate) void tick();
  handle = setIv(() => void tick(), intervalMs);

  return {
    stop() {
      if (stopped) return;
      stopped = true;
      if (handle !== undefined) clearIv(handle);
    },
    get ackOffset() {
      return ackOffset;
    },
    // Internal: advance the ack cursor from observed stream offsets.
    advanceAck(offset: bigint) {
      if (offset > ackOffset) ackOffset = offset;
    },
  } as InboxPoller & { advanceAck: (offset: bigint) => void };
}

/** Whether live notifications are arriving via the stream or the poll fallback. */
export type LiveStatus = "live" | "polling";

/** Options for [`subscribeNotificationsLive`]. */
export interface LiveInboxOptions {
  /** Restrict the feed to unread notifications (both stream-refetch + poll). */
  unreadOnly?: boolean;
  /** Poll period for the fallback, ms (default 5000). */
  pollIntervalMs?: number;
  /**
   * Fires with newly arrived notifications (deduped by id across BOTH the
   * stream and the poll paths), newest-first. Callers prepend these.
   */
  onNotifications: (fresh: Notification[]) => void;
  /** Fires whenever the transport mode flips (`"live"` ⇄ `"polling"`). */
  onStatus?: (status: LiveStatus) => void;
  /** Timer overrides (tests inject fakes); forwarded to [`pollInbox`]. */
  setInterval?: PollInboxOptions["setInterval"];
  clearInterval?: PollInboxOptions["clearInterval"];
  /** Fire the first poll immediately on fallback (default true). */
  immediate?: boolean;
}

/**
 * The 520 orchestrator: subscribe to `notification.events` and surface live
 * notifications; on a stream error, fall back to AckOffset polling. Dedups by id
 * across both paths so a notification is delivered at most once. Returns an
 * unsubscribe that tears down whichever path is active.
 *
 * On a `created`/`updated`/`acted` frame it refetches the inbox (the cheap,
 * always-available read) and emits the affected row; `read` frames advance the
 * ack cursor but emit nothing new (the row already exists locally).
 */
export function subscribeNotificationsLive(
  dc: DataClient,
  opts: LiveInboxOptions,
): Unsubscribe {
  const notifications = createClient(Notifications, dc.transport);
  const seen = new Set<string>();
  let unsubStream: Unsubscribe | undefined;
  let poller: (InboxPoller & { advanceAck?: (o: bigint) => void }) | undefined;
  let lastOffset = 0n;
  let stopped = false;

  const emitFresh = (rows: Notification[]) => {
    const fresh = rows.filter((n) => !seen.has(n.id));
    for (const n of fresh) seen.add(n.id);
    if (fresh.length > 0) opts.onNotifications(fresh);
  };

  const refetchFor = async (id: string) => {
    try {
      const res = await notifications.getInbox({
        unreadOnly: opts.unreadOnly ?? false,
        limit: 0,
      });
      if (stopped) return;
      // Emit the affected row (newest-first feed); dedup guards repeats.
      const hit = res.notifications.find((n) => n.id === id);
      emitFresh(hit ? [hit] : res.notifications);
    } catch {
      // A failed refetch is non-fatal; the next event or poll recovers it.
    }
  };

  const startPolling = () => {
    if (stopped || poller) return;
    opts.onStatus?.("polling");
    poller = pollInbox(dc, {
      ...(opts.pollIntervalMs !== undefined ? { intervalMs: opts.pollIntervalMs } : {}),
      ...(opts.unreadOnly !== undefined ? { unreadOnly: opts.unreadOnly } : {}),
      ...(opts.setInterval ? { setInterval: opts.setInterval } : {}),
      ...(opts.clearInterval ? { clearInterval: opts.clearInterval } : {}),
      ...(opts.immediate !== undefined ? { immediate: opts.immediate } : {}),
      onNotifications: emitFresh,
    });
    // Carry over the highest offset observed on the stream so the poller's first
    // ack reflects what we durably consumed before the stream dropped.
    poller.advanceAck?.(lastOffset);
  };

  opts.onStatus?.("live");
  unsubStream = subscribeNotifications(dc, {
    onFrame: (frame) => {
      if (frame.offset > lastOffset) lastOffset = frame.offset;
      if (frame.kind === "created" || frame.kind === "updated" || frame.kind === "acted") {
        void refetchFor(frame.id);
      }
      // `read` frames carry no new row; the offset bump above is enough.
    },
    onError: () => {
      // Stream died — drop to the polling fallback (dedup carries across).
      startPolling();
    },
  });

  return () => {
    stopped = true;
    unsubStream?.();
    poller?.stop();
  };
}

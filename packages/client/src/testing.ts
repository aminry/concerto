//! Test-only mock [`DataClient`] (Task 520). A high-fidelity, in-memory data
//! client — real connect plumbing for the unary RPCs (`GetInbox` / `MarkRead` /
//! `AckOffset`) via `createRouterTransport`, plus a hand-driven
//! `notification.events` stream the test can push frames onto. Used by the web
//! Playwright mock spec (no real Core) to prove live updates arrive without a
//! manual refresh, and reusable by any client test that wants a Core-free seam.
//!
//! NOT bundled into production: `apps/web` imports this only from its e2e setup
//! module, which is loaded behind a query flag.

import { create } from "@bufbuild/protobuf";
import { createRouterTransport } from "@connectrpc/connect";

import { type DataClient, dataClientFromTransport, type Unsubscribe } from "./data-client";
import { NOTIFICATION_EVENTS_SUBJECT, type NotificationFrame } from "./notifications";
import {
  type Notification,
  NotificationSchema,
  Notifications,
} from "./gen/concerto/v1/notifications_pb";
import { type Event, EventSchema, Streams } from "./gen/concerto/v1/streams_pb";

const enc = new TextEncoder();

/** A plain-object description of a notification (test fixtures stay JSON-ish). */
export type MockNotificationInit = {
  id: string;
  title?: string;
  body?: string;
  severity?: string;
  createdAtMs?: number;
};

/** Build a [`Notification`] from a plain init (epoch-ms numbers → bigint). */
export function mockNotification(init: MockNotificationInit): Notification {
  return create(NotificationSchema, {
    id: init.id,
    title: init.title ?? init.id,
    body: init.body ?? "",
    severity: init.severity ?? "low",
    createdAtMs: BigInt(init.createdAtMs ?? Date.now()),
  });
}

/** Build a `notification.events` [`Event`] carrying the FROZEN opaque JSON frame. */
export function mockNotificationEvent(
  offset: bigint | number,
  frame: { kind: NotificationFrame["kind"]; id: string; chip_id?: string; by_device_id?: string },
): Event {
  const { kind, ...rest } = frame;
  return create(EventSchema, {
    offset: BigInt(offset),
    checksOpaque: enc.encode(JSON.stringify({ kind: `notification.${kind}`, ...rest })),
  });
}

/** Handle to drive the mock from a test/harness. */
export interface MockDataClientHandle {
  /** The mock [`DataClient`] to feed into the app. */
  dataClient: DataClient;
  /**
   * Add a notification to the backing inbox so the NEXT `GetInbox` includes it
   * (newest-first; prepended). Returns the new feed length.
   */
  addNotification: (init: MockNotificationInit) => number;
  /** Push a frame to every live `notification.events` subscriber. */
  emit: (ev: Event) => void;
  /** Force every active stream subscriber into its error path (→ fallback). */
  failStream: (err?: unknown) => void;
}

/** Options for [`createMockDataClient`]. */
export interface MockDataClientOptions {
  /** The initial inbox feed (newest-first). */
  inbox?: MockNotificationInit[];
}

/**
 * Build a Core-free [`DataClient`] for tests. Unary RPCs run over a real
 * `createRouterTransport` router (so the typed clients work unchanged); the
 * `subscribe` seam is a local fan-out the test drives via the returned handle's
 * `emit` / `failStream`.
 */
export function createMockDataClient(opts: MockDataClientOptions = {}): MockDataClientHandle {
  const inbox: Notification[] = (opts.inbox ?? []).map(mockNotification);

  const transport = createRouterTransport((router) => {
    router.service(Notifications, {
      getInbox() {
        return { notifications: [...inbox] };
      },
      markRead(req) {
        const hit = inbox.find((n) => n.id === req.id);
        if (hit) hit.readAtMs = BigInt(Date.now());
        return {};
      },
    });
    router.service(Streams, {
      ackOffset() {
        return {};
      },
      // The router stream is unused (the mock fans out via `emit`), but a no-op
      // generator keeps the service shape complete.
      // eslint-disable-next-line require-yield
      async *subscribe() {
        // never yields; the mock delivers via the local `subscribe` below.
        await new Promise<void>(() => {});
      },
    });
  });

  type Sub = { onEvent: (ev: Event) => void; onError?: (err: unknown) => void };
  const subs = new Set<Sub>();

  const base = dataClientFromTransport(transport);
  const dataClient: DataClient = {
    transport: base.transport,
    subscribe(subject, onEvent, onError): Unsubscribe {
      if (subject !== NOTIFICATION_EVENTS_SUBJECT) {
        return base.subscribe(subject, onEvent, onError);
      }
      const sub: Sub = { onEvent, ...(onError ? { onError } : {}) };
      subs.add(sub);
      return () => subs.delete(sub);
    },
  };

  return {
    dataClient,
    addNotification(init) {
      inbox.unshift(mockNotification(init));
      return inbox.length;
    },
    emit(ev) {
      for (const s of subs) s.onEvent(ev);
    },
    failStream(err = new Error("mock stream error")) {
      for (const s of [...subs]) {
        subs.delete(s);
        s.onError?.(err);
      }
    },
  };
}

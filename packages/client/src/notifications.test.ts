import { describe, expect, it, vi } from "vitest";

import { create } from "@bufbuild/protobuf";
import { createRouterTransport } from "@connectrpc/connect";

import { dataClientFromTransport } from "./data-client";
import {
  NOTIFICATION_EVENTS_SUBJECT,
  parseNotificationFrame,
  pollInbox,
  subscribeNotifications,
  subscribeNotificationsLive,
} from "./notifications";
import {
  type Notification,
  NotificationSchema,
  Notifications,
} from "./gen/concerto/v1/notifications_pb";
import { EventSchema, Streams } from "./gen/concerto/v1/streams_pb";

const enc = new TextEncoder();

/** Build a `notification.events` Event carrying an opaque JSON frame. */
function frameEvent(offset: bigint, frame: Record<string, unknown>) {
  return create(EventSchema, {
    offset,
    checksOpaque: enc.encode(JSON.stringify(frame)),
  });
}

/** A minimal Notification for inbox fixtures. */
function notif(id: string): Notification {
  return create(NotificationSchema, { id, title: `n-${id}`, body: "", severity: "low" });
}

interface MockOpts {
  /** Frames the Subscribe stream yields before (optionally) erroring. */
  streamFrames?: ReturnType<typeof frameEvent>[];
  /** If set, the Subscribe stream throws this AFTER yielding `streamFrames`. */
  streamError?: Error;
  /** Successive GetInbox responses (cycled; last repeats). */
  inboxPages?: Notification[][];
  /** Sink for AckOffset calls (subject+offset) the client makes. */
  acks?: { subject: string; offset: bigint }[];
}

/** A high-fidelity MOCK transport — real connect plumbing, no live Core. */
function mockDataClient(opts: MockOpts) {
  let inboxCall = 0;
  const transport = createRouterTransport((router) => {
    router.service(Streams, {
      // eslint-disable-next-line require-yield -- generator may throw before yielding
      async *subscribe(req) {
        expect(req.subject).toBe(NOTIFICATION_EVENTS_SUBJECT);
        for (const ev of opts.streamFrames ?? []) {
          yield ev;
        }
        if (opts.streamError) throw opts.streamError;
      },
      ackOffset(req) {
        opts.acks?.push({ subject: req.subject, offset: req.offset });
        return {};
      },
    });
    router.service(Notifications, {
      getInbox() {
        const pages = opts.inboxPages ?? [[]];
        const page = pages[Math.min(inboxCall, pages.length - 1)] ?? [];
        inboxCall += 1;
        return { notifications: page };
      },
    });
  });
  return dataClientFromTransport(transport);
}

describe("parseNotificationFrame", () => {
  it("decodes the FROZEN opaque frame and strips the notification. prefix", () => {
    const f = parseNotificationFrame(frameEvent(7n, { kind: "notification.created", id: "n-1" }));
    expect(f).toEqual({ kind: "created", id: "n-1", offset: 7n });
  });

  it("carries chip_id / by_device_id on acted frames", () => {
    const f = parseNotificationFrame(
      frameEvent(9n, {
        kind: "notification.acted",
        id: "n-2",
        chip_id: "approve",
        by_device_id: "dev-1",
      }),
    );
    expect(f).toMatchObject({ kind: "acted", id: "n-2", chipId: "approve", byDeviceId: "dev-1" });
  });

  it("returns null for an event with no opaque frame", () => {
    expect(parseNotificationFrame(create(EventSchema, { offset: 1n }))).toBeNull();
  });
});

describe("subscribeNotifications (stream → callback)", () => {
  it("(a) delivers decoded stream frames to onFrame", async () => {
    const dc = mockDataClient({
      streamFrames: [
        frameEvent(1n, { kind: "notification.created", id: "a" }),
        frameEvent(2n, { kind: "notification.read", id: "b" }),
      ],
    });
    const got: string[] = [];
    await new Promise<void>((resolve) => {
      const unsub = subscribeNotifications(dc, {
        onFrame: (f) => {
          got.push(`${f.kind}:${f.id}@${f.offset}`);
          if (got.length === 2) {
            unsub();
            resolve();
          }
        },
      });
    });
    expect(got).toEqual(["created:a@1", "read:b@2"]);
  });
});

describe("pollInbox (AckOffset polling fallback)", () => {
  it("(b) dedups by id across ticks; (c) acks advance", async () => {
    const acks: { subject: string; offset: bigint }[] = [];
    const dc = mockDataClient({
      inboxPages: [
        [notif("1"), notif("2")], // tick 1
        [notif("2"), notif("3")], // tick 2: only "3" is fresh
        [notif("3")], // tick 3: nothing fresh
      ],
      acks,
    });

    // Drive ticks manually via an injected fake timer (held in a box so TS does
    // not narrow the callback-assigned value to `never`).
    const timer: { cb?: () => void } = {};
    const fresh: string[][] = [];
    const poller = pollInbox(dc, {
      immediate: true,
      setInterval: (cb) => {
        timer.cb = cb;
        return 0 as unknown as ReturnType<typeof setInterval>;
      },
      clearInterval: () => {},
      onNotifications: (rows) => fresh.push(rows.map((n) => n.id)),
    }) as ReturnType<typeof pollInbox> & { advanceAck: (o: bigint) => void };

    // tick 1 fired immediately.
    await vi.waitFor(() => expect(fresh.length).toBe(1));
    expect(fresh[0]).toEqual(["1", "2"]);

    // Pre-load an ack cursor (as subscribeNotificationsLive would after stream
    // frames) so the next tick emits an AckOffset.
    poller.advanceAck(42n);
    expect(poller.ackOffset).toBe(42n);

    // tick 2: only "3" is new.
    timer.cb?.();
    await vi.waitFor(() => expect(fresh.length).toBe(2));
    expect(fresh[1]).toEqual(["3"]);

    // tick 3: nothing fresh ⇒ no onNotifications call.
    timer.cb?.();
    await vi.waitFor(() =>
      expect(acks.filter((a) => a.subject === NOTIFICATION_EVENTS_SUBJECT).length).toBeGreaterThan(0),
    );
    expect(fresh.length).toBe(2);

    // (c) acks were sent at the advanced offset on the notification subject.
    expect(acks.every((a) => a.subject === NOTIFICATION_EVENTS_SUBJECT && a.offset === 42n)).toBe(
      true,
    );

    poller.stop();
  });

  it("stop() is idempotent and halts further ticks", async () => {
    const dc = mockDataClient({ inboxPages: [[notif("1")]] });
    let cleared = 0;
    const poller = pollInbox(dc, {
      immediate: false,
      setInterval: () => 0 as unknown as ReturnType<typeof setInterval>,
      clearInterval: () => {
        cleared += 1;
      },
      onNotifications: () => {},
    });
    poller.stop();
    poller.stop();
    expect(cleared).toBe(1);
  });
});

describe("subscribeNotificationsLive (orchestrator)", () => {
  it("starts live then falls back to polling on a stream error; dedups across both", async () => {
    const acks: { subject: string; offset: bigint }[] = [];
    const dc = mockDataClient({
      // Stream yields one created frame (offset 5) then errors → fallback.
      streamFrames: [frameEvent(5n, { kind: "notification.created", id: "live-1" })],
      streamError: new Error("stream dropped"),
      // GetInbox: first the live refetch (returns live-1), then the poll page
      // (live-1 again + poll-2). live-1 must NOT be re-emitted (dedup).
      inboxPages: [
        [notif("live-1")], // refetch for the stream's created frame
        [notif("live-1"), notif("poll-2")], // first poll tick
      ],
      acks,
    });

    const statuses: string[] = [];
    const fresh: string[] = [];
    const timer: { cb?: () => void } = {};

    const unsub = subscribeNotificationsLive(dc, {
      immediate: true,
      setInterval: (cb) => {
        timer.cb = cb;
        return 0 as unknown as ReturnType<typeof setInterval>;
      },
      clearInterval: () => {},
      onStatus: (s) => statuses.push(s),
      onNotifications: (rows) => fresh.push(...rows.map((n) => n.id)),
    });

    // It announced "live", saw the stream frame (→ live-1), then the stream
    // errored and it flipped to "polling".
    await vi.waitFor(() => expect(statuses).toContain("polling"));
    expect(statuses[0]).toBe("live");
    await vi.waitFor(() => expect(fresh).toContain("live-1"));

    // The poll fallback's first tick emits only the NEW row (live-1 deduped).
    await vi.waitFor(() => expect(fresh).toContain("poll-2"));
    expect(fresh.filter((id) => id === "live-1").length).toBe(1);

    // The carried-over stream offset (5) is acked once polling drives a tick.
    timer.cb?.();
    await vi.waitFor(() =>
      expect(acks.some((a) => a.subject === NOTIFICATION_EVENTS_SUBJECT && a.offset === 5n)).toBe(
        true,
      ),
    );

    unsub();
  });
});

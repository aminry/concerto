// Test-only mock DataClient with a DRIVABLE `Streams.Subscribe` server stream
// (Tasks 518). Built over `createRouterTransport` so the typed `Streams` client
// works unchanged; each `subscribe` call records its `{ subject, sinceOffset }`
// (so a spec can assert the re-subscribe used the right offset) and is fed frames
// the test pushes via `emit(subject, event)` / ended via `end(subject)` /
// `fail(subject)`. Not bundled into production.
import { create } from "@bufbuild/protobuf";
import { createRouterTransport } from "@connectrpc/connect";

import { type DataClient, dataClientFromTransport } from "@concerto/client";
import { type Event, EventSchema, Streams } from "@concerto/client/gen/concerto/v1/streams_pb";

/** A recorded subscribe call. */
export interface SubscribeCall {
  subject: string;
  sinceOffset: bigint;
}

/** A single open subscription's drive controls. */
interface OpenStream {
  push: (ev: Event) => void;
  end: () => void;
  fail: (err: unknown) => void;
}

/** The mock + its drive controls. */
export interface MockStreamDataClient {
  client: DataClient;
  /** Every subscribe call, in order. */
  subscribeCalls: SubscribeCall[];
  /** Push an event to the newest open stream for `subject`. */
  emit: (subject: string, ev: Event) => void;
  /** Cleanly end the newest open stream for `subject`. */
  end: (subject: string) => void;
  /** Error the newest open stream for `subject`. */
  fail: (subject: string, err?: unknown) => void;
  /** Count of currently-open streams for a subject. */
  openCount: (subject: string) => number;
}

/** Build an [`Event`] frame at `offset` (the only field the manager reads). */
export function mockEvent(offset: number | bigint): Event {
  return create(EventSchema, { offset: BigInt(offset) });
}

/** Build a Core-free DataClient with a drivable Streams.Subscribe stream. */
export function createMockStreamDataClient(): MockStreamDataClient {
  const subscribeCalls: SubscribeCall[] = [];
  // subject -> stack of open streams (newest last).
  const open = new Map<string, OpenStream[]>();

  const push = (subject: string, fn: (s: OpenStream) => void) => {
    const stack = open.get(subject);
    const s = stack?.[stack.length - 1];
    if (s) fn(s);
  };

  const transport = createRouterTransport((router) => {
    router.service(Streams, {
      ackOffset() {
        return {};
      },
      async *subscribe(req, ctx) {
        subscribeCalls.push({ subject: req.subject, sinceOffset: req.sinceOffset ?? 0n });

        const queue: Event[] = [];
        let resolveNext: (() => void) | undefined;
        let done = false;
        let failure: unknown;
        const wake = () => {
          resolveNext?.();
          resolveNext = undefined;
        };
        const handle: OpenStream = {
          push: (ev) => {
            queue.push(ev);
            wake();
          },
          end: () => {
            done = true;
            wake();
          },
          fail: (err) => {
            failure = err;
            done = true;
            wake();
          },
        };
        const stack = open.get(req.subject) ?? [];
        stack.push(handle);
        open.set(req.subject, stack);

        const remove = () => {
          const arr = open.get(req.subject);
          const i = arr?.indexOf(handle) ?? -1;
          if (arr && i >= 0) arr.splice(i, 1);
        };

        // End the stream if connect aborts the call (unsubscribe / session close).
        ctx.signal.addEventListener("abort", () => {
          done = true;
          wake();
        });

        try {
          for (;;) {
            while (queue.length > 0) {
              yield queue.shift()!;
            }
            if (failure) throw failure;
            if (done) return;
            await new Promise<void>((r) => {
              resolveNext = r;
            });
          }
        } finally {
          remove();
        }
      },
    });
  });

  return {
    client: dataClientFromTransport(transport),
    subscribeCalls,
    emit: (subject, ev) => push(subject, (s) => s.push(ev)),
    end: (subject) => push(subject, (s) => s.end()),
    fail: (subject, err = new Error("mock stream error")) => push(subject, (s) => s.fail(err)),
    openCount: (subject) => open.get(subject)?.length ?? 0,
  };
}

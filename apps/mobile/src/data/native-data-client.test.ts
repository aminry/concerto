// createNativeDataClient tests (Task 510, Tier-2). Proves the native adapter
// round-trips REAL generated proto types through the OPAQUE-BYTES mock module:
//   - unary: createClient(Notifications).getInbox encodes the request, the mock
//     decodes/re-encodes at the byte boundary, and the adapter decodes the
//     response back into a typed `InboxResponse`.
//   - server-streaming subscribe: a Streams.Subscribe stream of `Event` frames
//     pushed through the mock callback arrives decoded at the `subscribe` seam.
//
// The mock is a pure identity-codec passthrough (like 509): the test wires
// handlers that themselves use @bufbuild/protobuf, so a green test exercises the
// ADAPTER's encode/decode, not the mock's.
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { createClient } from "@connectrpc/connect";

import { nativeTransport } from "./native-data-client";
import type { ConcertoIrohModule, StreamEventCallback } from "../native/ConcertoIroh";

import {
  InboxFilterSchema,
  InboxResponseSchema,
  Notifications,
} from "@concerto/client/gen/concerto/v1/notifications_pb";
import {
  EventSchema,
  Streams,
  SubscribeRequestSchema,
} from "@concerto/client/gen/concerto/v1/streams_pb";

import { createNativeDataClient } from "./native-data-client";
import { createMockConcertoIroh } from "../native/mock-concerto-iroh";

const GET_INBOX = "/concerto.v1.Notifications/GetInbox";
const SUBSCRIBE = "/concerto.v1.Streams/Subscribe";

describe("createNativeDataClient", () => {
  it("round-trips a unary RPC (encode → rpcUnary → decode)", async () => {
    // The mock unary handler is the "Core": it DECODES the request bytes (proving
    // the adapter encoded a real proto), builds a typed response, and ENCODES it.
    let seenUnreadOnly: boolean | undefined;
    const module = createMockConcertoIroh({
      unary: {
        [GET_INBOX]: (payload) => {
          const req = fromBinary(InboxFilterSchema, payload);
          seenUnreadOnly = req.unreadOnly;
          const resp = create(InboxResponseSchema, {
            notifications: [
              {
                id: "ntf-1",
                title: "Agent finished",
                body: "done",
                severity: "low",
                createdAtMs: 1700000000000n,
              },
            ],
          });
          return toBinary(InboxResponseSchema, resp);
        },
      },
    });

    const handle = await module.openSession(
      { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
      new Uint8Array([1]),
    );
    const dc = createNativeDataClient(module, handle);
    const client = createClient(Notifications, dc.transport);

    const res = await client.getInbox({ unreadOnly: true, limit: 50 });

    expect(seenUnreadOnly).toBe(true);
    expect(res.notifications).toHaveLength(1);
    expect(res.notifications[0].id).toBe("ntf-1");
    expect(res.notifications[0].title).toBe("Agent finished");
    expect(res.notifications[0].createdAtMs).toBe(1700000000000n);
  });

  it("decodes an empty unary response", async () => {
    const module = createMockConcertoIroh({
      unary: {
        [GET_INBOX]: () => toBinary(InboxResponseSchema, create(InboxResponseSchema, {})),
      },
    });
    const handle = await module.openSession(
      { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
      new Uint8Array([1]),
    );
    const client = createClient(Notifications, createNativeDataClient(module, handle).transport);
    const res = await client.getInbox({});
    expect(res.notifications).toHaveLength(0);
  });

  it("subscribes to a server stream and decodes each Event frame", async () => {
    let seenSubject: string | undefined;
    const module = createMockConcertoIroh({
      stream: {
        [SUBSCRIBE]: (payload, cb) => {
          const req = fromBinary(SubscribeRequestSchema, payload);
          seenSubject = req.subject;
          // Push two decoded-then-encoded Event frames, then complete.
          for (const offset of [1n, 2n]) {
            const ev = create(EventSchema, { offset });
            cb.onEvent(toBinary(EventSchema, ev));
          }
          cb.onComplete();
          return () => {};
        },
      },
    });

    const handle = await module.openSession(
      { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
      new Uint8Array([1]),
    );
    const dc = createNativeDataClient(module, handle);

    const got: bigint[] = [];
    await new Promise<void>((resolve, reject) => {
      const unsub = dc.subscribe(
        "notification.events",
        (ev) => {
          got.push(ev.offset);
          if (got.length === 2) {
            unsub();
            resolve();
          }
        },
        reject,
      );
    });

    expect(seenSubject).toBe("notification.events");
    expect(got).toEqual([1n, 2n]);
  });

  it("surfaces a stream error to onError", async () => {
    const module = createMockConcertoIroh({
      stream: {
        [SUBSCRIBE]: (_payload, cb) => {
          cb.onError("core unreachable");
          return () => {};
        },
      },
    });
    const handle = await module.openSession(
      { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
      new Uint8Array([1]),
    );
    const dc = createNativeDataClient(module, handle);

    const err = await new Promise<unknown>((resolve) => {
      dc.subscribe(
        "notification.events",
        () => {},
        (e) => resolve(e),
      );
    });
    expect(String(err)).toContain("core unreachable");
  });

  it("does not open a native subscription when aborted before the first pull", async () => {
    // Regression (subscription leak on abort-before-first-pull): the native
    // subscription must be registered INSIDE the generator body so an abort that
    // lands before the consumer ever pulls a frame never calls `rpcStream` —
    // there is no native stream task + callback to leak until `closeSession`.
    const calls = { rpcStream: 0, cancelSubscription: 0 };
    const module = createMockConcertoIroh({
      stream: {
        [SUBSCRIBE]: (_payload, cb) => {
          cb.onComplete();
          return () => {};
        },
      },
    });
    // Wrap to count the native primitives the leak would touch.
    const tracking: ConcertoIrohModule = {
      ...module,
      rpcStream: (handle, method, payload, cb: StreamEventCallback) => {
        calls.rpcStream += 1;
        return module.rpcStream(handle, method, payload, cb);
      },
      cancelSubscription: (handle, subId) => {
        calls.cancelSubscription += 1;
        return module.cancelSubscription(handle, subId);
      },
    };

    const handle = await module.openSession(
      { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
      new Uint8Array([1]),
    );

    const ac = new AbortController();
    ac.abort(); // abort BEFORE constructing/iterating the stream
    const res = await nativeTransport(tracking, handle).stream(
      Streams.method.subscribe,
      ac.signal,
      undefined,
      undefined,
      (async function* () {
        yield create(SubscribeRequestSchema, { subject: "notification.events" });
      })(),
    );

    // The iterable exists but was never pulled, so `rpcStream` was never invoked:
    // nothing was registered, hence nothing to cancel or leak.
    expect(calls.rpcStream).toBe(0);
    expect(calls.cancelSubscription).toBe(0);

    // And pulling now (a late consumer of an already-aborted stream) tears down
    // cleanly via the in-generator subscribe → onAbort path without leaking.
    const iterator = res.message[Symbol.asyncIterator]();
    await iterator.next();
    expect(calls.rpcStream).toBe(1);
    expect(calls.cancelSubscription).toBeGreaterThanOrEqual(1);
  });

  it("cancels the native subscription on unsubscribe", async () => {
    let cancelled = false;
    const module = createMockConcertoIroh({
      stream: {
        [SUBSCRIBE]: (_payload, cb) => {
          // Emit one frame, then stay open until torn down.
          cb.onEvent(toBinary(EventSchema, create(EventSchema, { offset: 1n })));
          return () => {
            cancelled = true;
          };
        },
      },
    });
    const handle = await module.openSession(
      { endpointId: "ep", directAddrs: [], coreNoisePub: "00" },
      new Uint8Array([1]),
    );
    const dc = createNativeDataClient(module, handle);

    await new Promise<void>((resolve) => {
      const unsub = dc.subscribe("notification.events", () => {
        unsub();
        // Defer so the generator's finally (which cancels) runs.
        setTimeout(resolve, 0);
      });
    });
    expect(cancelled).toBe(true);
  });
});

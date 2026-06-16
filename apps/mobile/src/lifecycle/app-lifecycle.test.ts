// App lifecycle + resumable-stream tests (Task 518, Tier-2; design/16 §3.12 +
// §6.2). Proves:
//   - background -> the native session's close() is called (and streams abort),
//   - foreground -> a fresh session is opened + every subject re-subscribed,
//   - the re-subscribe carries a since_offset = the last offset observed before
//     background (replay only the missed frames),
//   - revalidate gating: a falsey revalidate blocks the reopen.
import { AppLifecycleController, type OpenedSession } from "./app-lifecycle";
import { StreamManager } from "./stream-manager";
import {
  createMockStreamDataClient,
  mockEvent,
  type MockStreamDataClient,
} from "./test-stream-client";

/** Let queued microtasks (the router's async stream pump) run. */
const flush = () => new Promise<void>((r) => setTimeout(r, 0));

describe("StreamManager", () => {
  it("tracks the highest offset and replays from it on reopen (since_offset)", async () => {
    const mock1 = createMockStreamDataClient();
    const seen: bigint[] = [];
    const mgr = new StreamManager();
    mgr.add({ subject: "notification.events", onEvent: (ev) => seen.push(ev.offset) });

    mgr.open(mock1.client);
    await flush();
    expect(mock1.subscribeCalls).toEqual([{ subject: "notification.events", sinceOffset: 0n }]);

    // Observe frames up to offset 7.
    mock1.emit("notification.events", mockEvent(3));
    mock1.emit("notification.events", mockEvent(7));
    await flush();
    expect(seen).toEqual([3n, 7n]);
    expect(mgr.offsetFor("notification.events")).toBe(7n);

    // Reopen against a fresh session — must resume FROM offset 7.
    const mock2 = createMockStreamDataClient();
    mgr.open(mock2.client);
    await flush();
    expect(mock2.subscribeCalls).toEqual([{ subject: "notification.events", sinceOffset: 7n }]);
  });

  it("close() aborts streams but retains the offset cursor", async () => {
    const mock = createMockStreamDataClient();
    const mgr = new StreamManager();
    mgr.add({ subject: "workspace.events", onEvent: () => {} });
    mgr.open(mock.client);
    await flush();
    mock.emit("workspace.events", mockEvent(5));
    await flush();
    expect(mock.openCount("workspace.events")).toBe(1);

    mgr.close();
    await flush();
    expect(mock.openCount("workspace.events")).toBe(0);
    expect(mgr.offsetFor("workspace.events")).toBe(5n);
  });
});

describe("AppLifecycleController", () => {
  /** A fake session-opener that records opens + closes. */
  function opener(mock: MockStreamDataClient) {
    const closes: number[] = [];
    let opens = 0;
    const openClient = jest.fn(async (): Promise<OpenedSession> => {
      opens += 1;
      const n = opens;
      return {
        client: mock.client,
        close: jest.fn(async () => {
          closes.push(n);
        }),
      };
    });
    return { openClient, closes, opens: () => opens };
  }

  it("foreground opens a session and subscribes; background closes it", async () => {
    const mock = createMockStreamDataClient();
    const o = opener(mock);
    const streams = new StreamManager();
    streams.add({ subject: "notification.events", onEvent: () => {} });
    const ctrl = new AppLifecycleController({ openClient: o.openClient, streams });

    await ctrl.foreground();
    await flush();
    expect(o.openClient).toHaveBeenCalledTimes(1);
    expect(ctrl.isOpen).toBe(true);
    expect(mock.subscribeCalls).toHaveLength(1);

    await ctrl.background();
    await flush();
    expect(o.closes).toEqual([1]); // the native session's close() ran
    expect(ctrl.isOpen).toBe(false);
    expect(mock.openCount("notification.events")).toBe(0); // streams aborted
  });

  it("foreground after a background re-subscribes with the since_offset", async () => {
    const mock = createMockStreamDataClient();
    const o = opener(mock);
    const streams = new StreamManager();
    streams.add({ subject: "notification.events", onEvent: () => {} });
    const ctrl = new AppLifecycleController({ openClient: o.openClient, streams });

    await ctrl.foreground();
    await flush();
    mock.emit("notification.events", mockEvent(11));
    await flush();
    await ctrl.background();
    await flush();
    await ctrl.foreground();
    await flush();

    // Two subscribe calls: the first from 0, the reopen from 11.
    expect(mock.subscribeCalls).toEqual([
      { subject: "notification.events", sinceOffset: 0n },
      { subject: "notification.events", sinceOffset: 11n },
    ]);
    expect(o.openClient).toHaveBeenCalledTimes(2);
  });

  it("is idempotent: a second foreground without a background does not reopen", async () => {
    const mock = createMockStreamDataClient();
    const o = opener(mock);
    const ctrl = new AppLifecycleController({ openClient: o.openClient, streams: new StreamManager() });
    await ctrl.foreground();
    await ctrl.foreground();
    await flush();
    expect(o.openClient).toHaveBeenCalledTimes(1);
  });

  it("revalidate gating: a falsey revalidate blocks the reopen", async () => {
    const mock = createMockStreamDataClient();
    const o = opener(mock);
    const ctrl = new AppLifecycleController({
      openClient: o.openClient,
      streams: new StreamManager(),
      revalidate: jest.fn(async () => false),
    });
    await ctrl.foreground();
    await flush();
    expect(o.openClient).not.toHaveBeenCalled();
    expect(ctrl.isOpen).toBe(false);
  });

  it("background with no open session is a no-op (no close)", async () => {
    const mock = createMockStreamDataClient();
    const o = opener(mock);
    const ctrl = new AppLifecycleController({ openClient: o.openClient, streams: new StreamManager() });
    await ctrl.background();
    expect(o.closes).toEqual([]);
  });
});

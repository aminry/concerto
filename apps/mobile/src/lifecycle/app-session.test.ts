// App-session glue tests (Tasks 516 + 518, Tier-2). Proves the wiring: foreground
// opens a session, resubscribes the default subjects, AND registers push; a
// failed push registration does not break the session; background closes it.
import { createAppSession, DEFAULT_SUBJECTS } from "./app-session";
import { StreamManager } from "./stream-manager";
import { createMockStreamDataClient } from "./test-stream-client";

const flush = () => new Promise<void>((r) => setTimeout(r, 0));

describe("createAppSession", () => {
  it("foreground opens a session, subscribes default subjects, and registers push", async () => {
    const mock = createMockStreamDataClient();
    let closed = 0;
    const open = jest.fn(async () => ({ client: mock.client, close: () => void closed++ }));
    const registerPush = jest.fn(async () => {});
    const streams = new StreamManager();
    for (const s of DEFAULT_SUBJECTS) streams.add({ subject: s, onEvent: () => {} });

    const { controller } = createAppSession({ streams, open, registerPush });
    await controller.foreground();
    await flush();

    expect(open).toHaveBeenCalledTimes(1);
    expect(registerPush).toHaveBeenCalledTimes(1);
    expect(registerPush).toHaveBeenCalledWith(mock.client);
    // Every default subject was subscribed.
    expect(new Set(mock.subscribeCalls.map((c) => c.subject))).toEqual(new Set(DEFAULT_SUBJECTS));

    await controller.background();
    await flush();
    expect(closed).toBe(1);
  });

  it("a failed push registration does not break the session", async () => {
    const mock = createMockStreamDataClient();
    const open = jest.fn(async () => ({ client: mock.client, close: () => {} }));
    const registerPush = jest.fn(async () => {
      throw new Error("permission denied");
    });
    const { controller } = createAppSession({ streams: new StreamManager(), open, registerPush });

    await controller.foreground();
    await flush();
    expect(controller.isOpen).toBe(true);
  });

  it("opens nothing when there is no Core (open returns null)", async () => {
    const open = jest.fn(async () => null);
    const registerPush = jest.fn(async () => {});
    const { controller } = createAppSession({ streams: new StreamManager(), open, registerPush });
    await controller.foreground();
    await flush();
    expect(controller.isOpen).toBe(false);
    expect(registerPush).not.toHaveBeenCalled();
  });
});

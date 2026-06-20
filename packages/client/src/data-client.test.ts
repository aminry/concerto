import { describe, expect, it } from "vitest";

import { createClient } from "@connectrpc/connect";

import { createConnectWebDataClient, dataClientFromTransport } from "./data-client";
import { Notifications } from "./gen/concerto/v1/notifications_pb";
import { Runtime } from "./gen/concerto/v1/runtime_pb";

describe("DataClient", () => {
  it("createConnectWebDataClient exposes a transport + subscribe", () => {
    const dc = createConnectWebDataClient({ baseUrl: "http://127.0.0.1:9999" });
    expect(dc.transport).toBeDefined();
    expect(typeof dc.subscribe).toBe("function");
  });

  it("typed service clients build off the transport (Runtime + Notifications)", () => {
    const dc = createConnectWebDataClient({ baseUrl: "http://127.0.0.1:9999" });
    // The generated service descriptors are usable with the DataClient's
    // transport — this is the whole point of the seam (no live server needed).
    const runtime = createClient(Runtime, dc.transport);
    const notifications = createClient(Notifications, dc.transport);
    expect(typeof runtime.getServerCapabilities).toBe("function");
    expect(typeof notifications.getInbox).toBe("function");
    expect(typeof notifications.actOnChip).toBe("function");
  });

  it("subscribe returns an unsubscribe handle that is safe to call", () => {
    const dc = createConnectWebDataClient({ baseUrl: "http://127.0.0.1:9999" });
    // No server is listening; the stream errors asynchronously into onError.
    // We only assert the synchronous API contract here (E2E is 519/520 Playwright).
    let unsub: () => void = () => {};
    expect(() => {
      unsub = dc.subscribe(
        "workspace.events",
        () => {},
        () => {},
      );
    }).not.toThrow();
    expect(() => unsub()).not.toThrow();
  });

  it("dataClientFromTransport wraps an arbitrary transport", () => {
    const dc = createConnectWebDataClient({ baseUrl: "http://127.0.0.1:9999" });
    const wrapped = dataClientFromTransport(dc.transport);
    expect(wrapped.transport).toBe(dc.transport);
  });
});

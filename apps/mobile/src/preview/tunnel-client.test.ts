// TunnelClient seam tests (Task 517): the fixture-backed mock resolves a typed
// TunnelInfo, honours `resolve`/`delayMs`/`failWith`, and `fixtureTunnel` is
// deterministic per id.
import { fixtureTunnel, mockTunnelClient } from "./tunnel-client";

describe("mockTunnelClient", () => {
  it("resolves a deterministic fixture tunnel URL for an id", async () => {
    const client = mockTunnelClient();
    const t = await client.startLocalhostTunnel("wa-aria");
    expect(t.id).toBe("wa-aria");
    expect(t.url).toBe("https://wa-aria.preview.concerto.localhost");
    expect(t.localPort).toBe(5173);
  });

  it("honours a custom resolve override", async () => {
    const client = mockTunnelClient({
      resolve: (id) => ({ id, url: `https://custom/${id}` }),
    });
    const t = await client.startLocalhostTunnel("x");
    expect(t.url).toBe("https://custom/x");
  });

  it("rejects with failWith (drives the error state)", async () => {
    const client = mockTunnelClient({ failWith: "no dev server" });
    await expect(client.startLocalhostTunnel("x")).rejects.toThrow("no dev server");
  });

  it("delays resolution by delayMs (drives the loading state)", async () => {
    jest.useFakeTimers();
    try {
      const client = mockTunnelClient({ delayMs: 500 });
      const p = client.startLocalhostTunnel("x");
      let settled = false;
      void p.then(() => {
        settled = true;
      });
      expect(settled).toBe(false);
      jest.advanceTimersByTime(500);
      await p;
      expect(settled).toBe(true);
    } finally {
      jest.useRealTimers();
    }
  });

  it("fixtureTunnel is deterministic", () => {
    expect(fixtureTunnel("abc")).toEqual(fixtureTunnel("abc"));
    expect(fixtureTunnel("abc").url).not.toBe(fixtureTunnel("xyz").url);
  });
});

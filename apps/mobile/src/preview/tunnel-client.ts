// Localhost preview tunnel seam (Task 517). The Preview screen requests a public
// tunnel URL for a workarea's dev server and renders it in a WebView.
//
// IMPORTANT: there is NO generated proto service for tunnels/preview in
// @concerto/client yet (verified: nothing matching Tunnel/Preview/StartLocalhost
// in packages/client/src/gen). So this file defines a small, transport-agnostic
// `TunnelClient` facade with a typed fixture-backed mock — the exact same seam
// pattern as `WorkspacesClient` (Task 513). When the Core grows a
// `StartLocalhostTunnel` RPC, the live implementation (a later task) swaps the
// factory body to `createClient(<Service>, dc.transport)`; the screen, which
// takes the seam as a prop, does not change.

/** The result of starting (or resolving) a localhost preview tunnel. */
export interface TunnelInfo {
  /** The id of the workarea / dev-server this tunnel fronts. */
  id: string;
  /** The public URL to load in the WebView (e.g. `https://abc123.preview.concerto.dev`). */
  url: string;
  /** The local port being tunneled (display only), if known. */
  localPort?: number;
  /** ISO timestamp the tunnel was established (display only), if known. */
  startedAt?: string;
}

/** The Preview screen's data contract (Task 517). */
export interface TunnelClient {
  /**
   * Start (or resolve an existing) localhost preview tunnel for `id` and return
   * its public URL. Rejects when the Core can't establish a tunnel (no dev
   * server, tunnel quota, transport down) — drives the screen's error state.
   */
  startLocalhostTunnel(id: string): Promise<TunnelInfo>;
}

/** Options for [`mockTunnelClient`] (mirrors `MockOptions` in workspaces-client). */
export interface MockTunnelOptions {
  /** If set, `startLocalhostTunnel` rejects with this message (drives error UI). */
  failWith?: string;
  /** Artificial delay (ms) before resolving — lets a test observe the loading state. */
  delayMs?: number;
  /** Override the resolved tunnel (else a deterministic fixture is derived from `id`). */
  resolve?: (id: string) => TunnelInfo;
}

/** A deterministic fixture tunnel for an id (used by the app shell + tests). */
export function fixtureTunnel(id: string): TunnelInfo {
  return {
    id,
    url: `https://${id}.preview.concerto.localhost`,
    localPort: 5173,
    startedAt: "2026-06-16T12:00:00.000Z",
  };
}

/**
 * Build a fixture-backed [`TunnelClient`] for tests + the pre-live-transport app
 * shell. Resolves a deterministic [`TunnelInfo`] (or `opts.resolve(id)`); set
 * `failWith` to drive the error state, `delayMs` to observe loading.
 */
export function mockTunnelClient(opts: MockTunnelOptions = {}): TunnelClient {
  return {
    startLocalhostTunnel(id: string): Promise<TunnelInfo> {
      const value = () => (opts.resolve ? opts.resolve(id) : fixtureTunnel(id));
      if (opts.failWith) return Promise.reject(new Error(opts.failWith));
      if (opts.delayMs && opts.delayMs > 0) {
        return new Promise((resolve) => setTimeout(() => resolve(value()), opts.delayMs));
      }
      return Promise.resolve().then(value);
    },
  };
}

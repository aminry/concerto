// Cellular detection → "lite mode" (Task 518; design/16 §3.12 — lite-mode
// cellular streaming). On a metered cellular link the app should reduce / pause
// heavy streaming (large session-io tails, eager refetches) and lean on the
// cheap polling + on-demand fetches; on Wi-Fi / Ethernet it streams freely.
//
// A thin, INJECTABLE seam over `expo-network` (a NATIVE module — the real radio
// state is Tier-3). The seam exposes the current state + a change subscription so
// the lite-mode flag tracks connectivity live. Pure Tier-2 with a fake api.
import * as Network from "expo-network";

/** The connection class we care about for lite-mode. */
export type ConnectionType = "wifi" | "cellular" | "ethernet" | "other" | "none";

/** A network snapshot (narrowed from `expo-network`'s NetworkState). */
export interface NetworkSnapshot {
  type: ConnectionType;
  isConnected: boolean;
}

/** A subscription handle. */
export interface NetworkSubscription {
  remove(): void;
}

/** The narrow `expo-network` surface the lite-mode logic needs. */
export interface NetworkApi {
  getNetworkStateAsync(): Promise<NetworkSnapshot>;
  /** Subscribe to connectivity changes. Returns a remover. */
  addNetworkStateListener(listener: (s: NetworkSnapshot) => void): NetworkSubscription;
}

/** Map an `expo-network` NetworkStateType string to our [`ConnectionType`]. */
export function mapNetworkType(type: unknown): ConnectionType {
  switch (type) {
    case Network.NetworkStateType.WIFI:
      return "wifi";
    case Network.NetworkStateType.CELLULAR:
      return "cellular";
    case Network.NetworkStateType.ETHERNET:
      return "ethernet";
    case Network.NetworkStateType.NONE:
      return "none";
    default:
      return "other";
  }
}

/** The real `expo-network`-backed implementation (Tier-3 radio). */
export function defaultNetworkApi(): NetworkApi {
  return {
    getNetworkStateAsync: async () => {
      const s = await Network.getNetworkStateAsync();
      return {
        type: mapNetworkType(s.type),
        isConnected: s.isConnected ?? false,
      };
    },
    addNetworkStateListener: (listener) => {
      const sub = Network.addNetworkStateListener((s) =>
        listener({
          type: mapNetworkType(s.type),
          isConnected: s.isConnected ?? false,
        }),
      );
      return { remove: () => sub.remove() };
    },
  };
}

/**
 * Whether a given network snapshot should put the app in LITE MODE. Lite mode is
 * ON for a metered cellular link; OFF for wifi / ethernet / other / disconnected
 * (no point throttling when there's nothing to stream, and "other"/VPN is treated
 * as unmetered — conservative: we only throttle a definite cellular link).
 */
export function isLiteMode(snapshot: NetworkSnapshot): boolean {
  return snapshot.type === "cellular";
}

/** Options for [`watchLiteMode`]. */
export interface WatchLiteModeOptions {
  /** The network seam (defaults to the real `expo-network` module). */
  api?: NetworkApi;
  /** Fires whenever the lite-mode flag flips (and once with the initial value). */
  onChange: (lite: boolean, snapshot: NetworkSnapshot) => void;
}

/** Handle returned by [`watchLiteMode`]. */
export interface LiteModeWatcher {
  /** Stop watching. Idempotent. */
  stop(): void;
}

/**
 * Watch connectivity and drive the lite-mode flag. Emits the initial value once
 * (after the first `getNetworkStateAsync`) and then on every change. Returns a
 * handle to stop. The flag only emits when it actually flips (deduped).
 */
export function watchLiteMode(opts: WatchLiteModeOptions): LiteModeWatcher {
  const api = opts.api ?? defaultNetworkApi();
  let stopped = false;
  let last: boolean | undefined;

  const emit = (snapshot: NetworkSnapshot) => {
    if (stopped) return;
    const lite = isLiteMode(snapshot);
    if (lite === last) return;
    last = lite;
    opts.onChange(lite, snapshot);
  };

  void api.getNetworkStateAsync().then(emit);
  const sub = api.addNetworkStateListener(emit);

  return {
    stop() {
      if (stopped) return;
      stopped = true;
      sub.remove();
    },
  };
}

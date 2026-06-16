// Jest setup (Task 508 + Task 511). `@testing-library/react-native` auto-registers
// its matchers (`toBeOnTheScreen`, etc.) on import since v12.4 — importing it here
// makes them available to every spec without per-file boilerplate.
import "@testing-library/react-native";

// ── Native-module mocks (Task 511) ──────────────────────────────────────────
// `expo-secure-store` and `expo-camera` are NATIVE modules: their real behaviour
// (Keychain/Keystore, the camera) is Tier-3, unavailable in the jest runtime.
// We mock them here so the pairing logic + screens run Tier-2. Each mock mirrors
// only the surface our code uses.

// expo-secure-store: a per-test in-memory string map. `__resetSecureStore` lets a
// test start from a clean slate (the store module has no reset of its own).
jest.mock("expo-secure-store", () => {
  const store = new Map<string, string>();
  return {
    __esModule: true,
    __resetSecureStore: () => store.clear(),
    __dumpSecureStore: () => new Map(store),
    setItemAsync: jest.fn(async (key: string, value: string) => {
      store.set(key, value);
    }),
    getItemAsync: jest.fn(async (key: string) => (store.has(key) ? store.get(key)! : null)),
    deleteItemAsync: jest.fn(async (key: string) => {
      store.delete(key);
    }),
    isAvailableAsync: jest.fn(async () => true),
  };
});

// expo-camera: a minimal mock. `CameraView` renders nothing; `useCameraPermissions`
// returns a granted permission by default. A spec can override these per-test.
jest.mock("expo-camera", () => {
  const React = require("react");
  return {
    __esModule: true,
    CameraView: jest.fn((props: Record<string, unknown>) =>
      React.createElement("CameraView", props),
    ),
    useCameraPermissions: jest.fn(() => [
      { granted: true, canAskAgain: true, status: "granted" },
      jest.fn(async () => ({ granted: true, status: "granted" })),
    ]),
  };
});

// react-native-webview (Task 517): the WebView is a NATIVE view — real page
// loads are Tier-3. The mock renders a host element preserving `source`/`testID`
// + the load callbacks so specs can assert the URL and drive onLoad*/onError
// (e.g. `props.onLoadEnd?.()`) deterministically.
jest.mock("react-native-webview", () => {
  const React = require("react");
  return {
    __esModule: true,
    WebView: jest.fn((props: Record<string, unknown>) =>
      React.createElement("WebView", props),
    ),
  };
});

// ── Task 516 + 518 native-module mocks ──────────────────────────────────────
// expo-notifications / expo-local-authentication / expo-network are NATIVE
// modules (push registration + system UI, biometric prompt, the radio). Real
// behaviour is Tier-3. The push/lifecycle UNITS inject their own seams (see
// `src/push/expo-notifications.ts`, `biometric-gate.ts`, `src/net/lite-mode.ts`),
// so these module mocks only keep an accidental real-module import from crashing
// the jest runtime; the seam-default factories (`defaultNotificationsApi`, …)
// resolve against them.

// expo-notifications: permission granted, a deterministic token, no-op category +
// listener registration. `DEFAULT_ACTION_IDENTIFIER` mirrors the real constant.
jest.mock("expo-notifications", () => ({
  __esModule: true,
  DEFAULT_ACTION_IDENTIFIER: "expo.modules.notifications.actions.DEFAULT",
  getPermissionsAsync: jest.fn(async () => ({ granted: true })),
  requestPermissionsAsync: jest.fn(async () => ({ granted: true })),
  getExpoPushTokenAsync: jest.fn(async () => ({ data: "ExponentPushToken[mock]" })),
  setNotificationCategoryAsync: jest.fn(async () => ({})),
  addNotificationResponseReceivedListener: jest.fn(() => ({ remove: jest.fn() })),
  addNotificationReceivedListener: jest.fn(() => ({ remove: jest.fn() })),
}));

// expo-local-authentication: hardware present + enrolled, prompt succeeds.
jest.mock("expo-local-authentication", () => ({
  __esModule: true,
  hasHardwareAsync: jest.fn(async () => true),
  isEnrolledAsync: jest.fn(async () => true),
  authenticateAsync: jest.fn(async () => ({ success: true })),
}));

// expo-network: wifi + connected by default; no-op change listener.
jest.mock("expo-network", () => ({
  __esModule: true,
  NetworkStateType: {
    NONE: "NONE",
    UNKNOWN: "UNKNOWN",
    CELLULAR: "CELLULAR",
    WIFI: "WIFI",
    BLUETOOTH: "BLUETOOTH",
    ETHERNET: "ETHERNET",
    WIMAX: "WIMAX",
    VPN: "VPN",
    OTHER: "OTHER",
  },
  getNetworkStateAsync: jest.fn(async () => ({ type: "WIFI", isConnected: true })),
  addNetworkStateListener: jest.fn(() => ({ remove: jest.fn() })),
}));

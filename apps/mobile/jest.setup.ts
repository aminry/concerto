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

// Push registration tests (Task 516, Tier-2). Proves the FROZEN flow:
//   register -> permission -> token -> Devices.UpdateDevicePushToken (mock
//   DataClient) with push_platform "expo"; a denied permission degrades to
//   { registered: false } without calling the RPC; the lock-screen action
//   category (Approve/Dismiss) is registered.
//
// The native push module is a hand-built fake (the `NotificationsApi` seam); the
// DataClient is the router-backed mock. No native code — the real push is Tier-3.
import type { NotificationsApi, PushCategoryAction } from "./expo-notifications";
import {
  APPROVAL_CATEGORY_ID,
  APPROVE_ACTION_ID,
  DISMISS_ACTION_ID,
  PUSH_PLATFORM_EXPO,
  registerForPush,
} from "./register";
import { createMockPushDataClient } from "./test-data-client";

/** A configurable fake of the `expo-notifications` seam, recording calls. */
function fakeApi(over: Partial<NotificationsApi> & { granted?: boolean; token?: string } = {}) {
  const categoryCalls: { id: string; actions: PushCategoryAction[] }[] = [];
  let permissionPrompted = false;
  const api: NotificationsApi = {
    getPermissionsAsync: jest.fn(async () => ({ granted: over.granted ?? false })),
    requestPermissionsAsync: jest.fn(async () => {
      permissionPrompted = true;
      return { granted: over.granted ?? true };
    }),
    getExpoPushTokenAsync: jest.fn(async () => ({ data: over.token ?? "ExponentPushToken[t]" })),
    setNotificationCategoryAsync: jest.fn(async (id, actions) => {
      categoryCalls.push({ id, actions });
      return {};
    }),
    addNotificationResponseReceivedListener: jest.fn(() => ({ remove: jest.fn() })),
    DEFAULT_ACTION_IDENTIFIER: "expo.modules.notifications.actions.DEFAULT",
    ...over,
  };
  return { api, categoryCalls, wasPrompted: () => permissionPrompted };
}

describe("registerForPush", () => {
  it("permission -> token -> UpdateDevicePushToken(expo) on the active Core", async () => {
    const { api, categoryCalls } = fakeApi({ granted: true, token: "ExponentPushToken[abc]" });
    const dc = createMockPushDataClient();

    const result = await registerForPush({ api, client: dc.client, deviceId: "deadbeef" });

    expect(result.registered).toBe(true);
    expect(result.token).toBe("ExponentPushToken[abc]");

    // The Core RPC fired with the frozen shape.
    expect(dc.pushTokenCalls).toHaveLength(1);
    expect(dc.pushTokenCalls[0]).toEqual({
      deviceId: "deadbeef",
      pushToken: "ExponentPushToken[abc]",
      pushPlatform: PUSH_PLATFORM_EXPO,
    });
    expect(PUSH_PLATFORM_EXPO).toBe("expo");

    // The lock-screen Approve/Dismiss category was registered.
    expect(categoryCalls).toHaveLength(1);
    expect(categoryCalls[0].id).toBe(APPROVAL_CATEGORY_ID);
    expect(categoryCalls[0].actions.map((a) => a.identifier)).toEqual([
      APPROVE_ACTION_ID,
      DISMISS_ACTION_ID,
    ]);
    // Approve opens the app to the foreground (so the biometric gate can run).
    expect(categoryCalls[0].actions[0].options?.opensAppToForeground).toBe(true);
  });

  it("does NOT re-prompt when permission is already granted", async () => {
    const { api, wasPrompted } = fakeApi({ granted: true });
    const dc = createMockPushDataClient();
    await registerForPush({ api, client: dc.client, deviceId: "d1" });
    expect(wasPrompted()).toBe(false);
    expect(api.requestPermissionsAsync).not.toHaveBeenCalled();
  });

  it("prompts when undetermined, then registers", async () => {
    // getPermissions -> not granted; requestPermissions -> granted.
    const { api } = fakeApi();
    (api.getPermissionsAsync as jest.Mock).mockResolvedValueOnce({ granted: false });
    (api.requestPermissionsAsync as jest.Mock).mockResolvedValueOnce({ granted: true });
    const dc = createMockPushDataClient();

    const result = await registerForPush({ api, client: dc.client, deviceId: "d2" });

    expect(api.requestPermissionsAsync).toHaveBeenCalledTimes(1);
    expect(result.registered).toBe(true);
    expect(dc.pushTokenCalls).toHaveLength(1);
  });

  it("degrades to { registered: false } on a denied permission (no RPC, no token)", async () => {
    const { api } = fakeApi();
    (api.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: false });
    (api.requestPermissionsAsync as jest.Mock).mockResolvedValue({ granted: false });
    const dc = createMockPushDataClient();

    const result = await registerForPush({ api, client: dc.client, deviceId: "d3" });

    expect(result).toEqual({ registered: false, reason: "permission-denied" });
    expect(api.getExpoPushTokenAsync).not.toHaveBeenCalled();
    expect(dc.pushTokenCalls).toHaveLength(0);
  });

  it("forwards a projectId to getExpoPushTokenAsync", async () => {
    const { api } = fakeApi({ granted: true });
    const dc = createMockPushDataClient();
    await registerForPush({ api, client: dc.client, deviceId: "d4", projectId: "proj-1" });
    expect(api.getExpoPushTokenAsync).toHaveBeenCalledWith({ projectId: "proj-1" });
  });
});

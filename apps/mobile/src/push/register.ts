// Push registration (Task 516, over Task 503's `Devices.UpdateDevicePushToken`
// backend + Task 511's multi-Core keystore). The flow (design/16 §3.12):
//
//   1. ensure push permission (prompt once if undetermined),
//   2. fetch the Expo push token (`getExpoPushTokenAsync`),
//   3. register a lock-screen action category (Approve/Dismiss chips),
//   4. call `Devices.UpdateDevicePushToken({ device_id, push_token,
//      push_platform: "expo" })` over the active Core's DataClient.
//
// `push_platform` is "expo" — the snake_case value the Core's `devices.push_platform`
// CHECK accepts (apns | fcm | expo; see Task 503 / devices_pb). The device id is
// the per-Core `deviceIdHex` recorded at pair time (`core-store`).
//
// Both the notifications module and the DataClient are INJECTED so this is a pure
// Tier-2 unit; the real native push + a live Core are Tier-3.
import { createClient } from "@connectrpc/connect";
import type { DataClient } from "@concerto/client";
import { Devices } from "@concerto/client/gen/concerto/v1/devices_pb";

import { activeCore } from "../pairing/core-store";
import {
  defaultNotificationsApi,
  type NotificationsApi,
  type PushCategoryAction,
} from "./expo-notifications";

/** The `push_platform` value for an Expo push token (Core CHECK: apns|fcm|expo). */
export const PUSH_PLATFORM_EXPO = "expo";

/** The lock-screen action category id our notifications are tagged with. */
export const APPROVAL_CATEGORY_ID = "concerto.approval";

/** Chip ids the lock-screen category exposes (mapped to chip dispatch on tap). */
export const APPROVE_ACTION_ID = "approve";
export const DISMISS_ACTION_ID = "dismiss";

/** The Approve/Dismiss action chips shown on a tool-approval notification. */
export const APPROVAL_CATEGORY_ACTIONS: PushCategoryAction[] = [
  // Approve opens the app to the foreground so the biometric gate can run before
  // the chip is dispatched (you should never approve a tool from the lock screen
  // without unlocking — design/16 §3.12).
  {
    identifier: APPROVE_ACTION_ID,
    buttonTitle: "Approve",
    options: { opensAppToForeground: true },
  },
  // Dismiss is a low-stakes background action (no app launch needed).
  {
    identifier: DISMISS_ACTION_ID,
    buttonTitle: "Dismiss",
    options: { opensAppToForeground: false, isDestructive: true },
  },
];

/** Options for [`registerForPush`]. */
export interface RegisterForPushOptions {
  /** The notifications seam (defaults to the real `expo-notifications` module). */
  api?: NotificationsApi;
  /**
   * The active Core's DataClient + device id. Injected so a test supplies a mock
   * DataClient; production passes the live one from `appDataClient()`.
   */
  client: DataClient;
  /** This device's id (hex) for the Core — the per-Core `deviceIdHex`. */
  deviceId: string;
  /** Optional EAS project id forwarded to `getExpoPushTokenAsync`. */
  projectId?: string;
}

/** The outcome of [`registerForPush`]. */
export interface RegisterForPushResult {
  /** True iff permission was granted and the token was registered with the Core. */
  registered: boolean;
  /** The Expo push token, if one was obtained. */
  token?: string;
  /** Why registration did not complete (permission denied / no token). */
  reason?: "permission-denied";
}

/**
 * Register this device for push and report the Expo token to the active Core via
 * `Devices.UpdateDevicePushToken`. Idempotent on the Core side (re-registering
 * just rewrites the row). Returns `{ registered: false, reason }` without throwing
 * when permission is denied so callers can degrade gracefully (polling fallback).
 */
export async function registerForPush(
  opts: RegisterForPushOptions,
): Promise<RegisterForPushResult> {
  const api = opts.api ?? defaultNotificationsApi();

  // 1. Permission — only prompt if not already granted.
  let granted = (await api.getPermissionsAsync()).granted;
  if (!granted) {
    granted = (await api.requestPermissionsAsync()).granted;
  }
  if (!granted) {
    return { registered: false, reason: "permission-denied" };
  }

  // 2. Register the lock-screen action category (best-effort; non-fatal).
  await api.setNotificationCategoryAsync(APPROVAL_CATEGORY_ID, APPROVAL_CATEGORY_ACTIONS);

  // 3. Token.
  const { data: token } = await api.getExpoPushTokenAsync(
    opts.projectId ? { projectId: opts.projectId } : undefined,
  );

  // 4. Report to the Core (Task 503 RPC).
  const devices = createClient(Devices, opts.client.transport);
  await devices.updateDevicePushToken({
    deviceId: opts.deviceId,
    pushToken: token,
    pushPlatform: PUSH_PLATFORM_EXPO,
  });

  return { registered: true, token };
}

/**
 * Convenience: register using the ACTIVE Core's device id (read from the keystore
 * registry). Returns `null` when no Core is paired (nothing to register against).
 */
export async function registerActiveDeviceForPush(opts: {
  api?: NotificationsApi;
  client: DataClient;
  projectId?: string;
}): Promise<RegisterForPushResult | null> {
  const core = await activeCore();
  if (!core) return null;
  return registerForPush({
    ...(opts.api ? { api: opts.api } : {}),
    client: opts.client,
    deviceId: core.deviceIdHex,
    ...(opts.projectId ? { projectId: opts.projectId } : {}),
  });
}

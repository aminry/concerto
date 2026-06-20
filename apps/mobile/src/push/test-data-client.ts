// Test-only mock DataClient for the push/wakeup units (Tasks 516/518). Real
// connect plumbing via `createRouterTransport` for the unary RPCs the push
// pipeline drives — `Devices.UpdateDevicePushToken`, `Notifications.GetNotification`,
// `Notifications.ActOnChip` — recording each call so a spec can assert the exact
// request. Not bundled into production (lives under a *.test-only seam imported
// only by specs).
import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { createRouterTransport } from "@connectrpc/connect";

import { type DataClient, dataClientFromTransport } from "@concerto/client";
import { Devices } from "@concerto/client/gen/concerto/v1/devices_pb";
import {
  NotificationSchema,
  Notifications,
} from "@concerto/client/gen/concerto/v1/notifications_pb";

/** A recorded UpdateDevicePushToken request. */
export interface PushTokenCall {
  deviceId: string;
  pushToken: string;
  pushPlatform: string;
}

/** A recorded GetNotification request. */
export interface GetNotificationCall {
  id: string;
  deviceId: string;
}

/** A recorded ActOnChip request. */
export interface ActOnChipCall {
  notificationId: string;
  chipId: string;
  deviceId: string;
}

/** The mock DataClient + the calls it recorded. */
export interface MockPushDataClient {
  client: DataClient;
  pushTokenCalls: PushTokenCall[];
  getNotificationCalls: GetNotificationCall[];
  actOnChipCalls: ActOnChipCall[];
}

/** Options controlling the mock's responses. */
export interface MockPushDataClientOptions {
  /** The notification `GetNotification` resolves to (default a minimal one). */
  notification?: MessageInitShape<typeof NotificationSchema> & { id: string };
  /** The ActOnChip response (default: navigate, not already-resolved). */
  actOnChip?: { alreadyResolved?: boolean; dispatchKind?: string; dispatchArg?: string };
}

/** Build a Core-free DataClient that routes the push pipeline's RPCs. */
export function createMockPushDataClient(
  opts: MockPushDataClientOptions = {},
): MockPushDataClient {
  const pushTokenCalls: PushTokenCall[] = [];
  const getNotificationCalls: GetNotificationCall[] = [];
  const actOnChipCalls: ActOnChipCall[] = [];

  const transport = createRouterTransport((router) => {
    router.service(Devices, {
      updateDevicePushToken(req) {
        pushTokenCalls.push({
          deviceId: req.deviceId,
          pushToken: req.pushToken,
          pushPlatform: req.pushPlatform,
        });
        return {};
      },
    });
    router.service(Notifications, {
      getNotification(req) {
        getNotificationCalls.push({ id: req.id, deviceId: req.deviceId });
        return create(NotificationSchema, {
          title: "fetched",
          body: "full body fetched post-wakeup",
          severity: "high",
          ...(opts.notification ?? {}),
          // The fetched notification always carries the requested id.
          id: opts.notification?.id ?? req.id,
        });
      },
      actOnChip(req) {
        actOnChipCalls.push({
          notificationId: req.notificationId,
          chipId: req.chipId,
          deviceId: req.deviceId,
        });
        return {
          alreadyResolved: opts.actOnChip?.alreadyResolved ?? false,
          dispatchKind: opts.actOnChip?.dispatchKind ?? "navigate",
          dispatchArg: opts.actOnChip?.dispatchArg ?? "workarea/abc",
        };
      },
    });
  });

  return {
    client: dataClientFromTransport(transport),
    pushTokenCalls,
    getNotificationCalls,
    actOnChipCalls,
  };
}

// A thin, INJECTABLE seam over `expo-notifications` (Task 516; design/16 §3.12,
// the FROZEN D6 ID-only push payload). `expo-notifications` is a NATIVE module:
// its real behaviour (the system push registration + the OS notification UI) is
// Tier-3, unavailable in jest. This module narrows the surface our code uses to a
// small `NotificationsApi` interface so:
//
//   - production code calls `defaultNotificationsApi()` (the real module), and
//   - tests pass a hand-built fake (no native code, no jest module mock needed for
//     the unit under test).
//
// Keeping the seam here (rather than calling `expo-notifications` directly from
// `register.ts` / `wakeup.ts`) is what makes the push pipeline a pure Tier-2 unit.
import * as Notifications from "expo-notifications";

/** The frozen D6 ID-only push payload (design/16 §3.12). All values are strings
 *  (FCM/APNs/Expo data payloads are string maps), so the Core sends only the
 *  notification id + kind + source — never the body. The device does the
 *  POST-WAKEUP FETCH over the E2EE channel to get the full notification. */
export interface IdOnlyPayload {
  /** The affected notification id (ULID) — the only thing needed to refetch. */
  notification_id: string;
  /** The notification kind (e.g. "tool_approval_needed") — drives the category. */
  kind: string;
  /** Where the push originated (e.g. the Core's endpoint id) — for routing. */
  source: string;
}

/** A subscription handle (mirrors expo-modules-core's `EventSubscription`). */
export interface PushSubscription {
  remove(): void;
}

/**
 * The narrow `expo-notifications` surface the push pipeline needs. Mirrors the
 * real module 1:1 so `defaultNotificationsApi()` is a straight passthrough and a
 * test fake is trivial to build.
 */
export interface NotificationsApi {
  /** Current permission status (`granted`). */
  getPermissionsAsync(): Promise<{ granted: boolean }>;
  /** Prompt for push permission; resolves the resulting status. */
  requestPermissionsAsync(): Promise<{ granted: boolean }>;
  /** Resolve the Expo push token string (the `push_token` we register). */
  getExpoPushTokenAsync(opts?: { projectId?: string }): Promise<{ data: string }>;
  /** Register a lock-screen action category (Approve/Dismiss chips). */
  setNotificationCategoryAsync(
    identifier: string,
    actions: PushCategoryAction[],
  ): Promise<unknown>;
  /** Fire when the user taps a notification or one of its action chips. */
  addNotificationResponseReceivedListener(
    listener: (response: NotificationResponseLike) => void,
  ): PushSubscription;
  /** The action id the OS sends when the body (not a chip) is tapped. */
  readonly DEFAULT_ACTION_IDENTIFIER: string;
}

/** One lock-screen action chip in a category. */
export interface PushCategoryAction {
  /** Stable id echoed back on the response's `actionIdentifier`. */
  identifier: string;
  /** Button label shown on the lock screen. */
  buttonTitle: string;
  /** iOS option toggles (e.g. `opensAppToForeground`). */
  options?: { opensAppToForeground?: boolean; isDestructive?: boolean };
}

/** The shape of a tapped-notification response we read (subset of the real type). */
export interface NotificationResponseLike {
  /** The chip id, or `DEFAULT_ACTION_IDENTIFIER` for a body tap. */
  actionIdentifier: string;
  notification: {
    request: {
      identifier: string;
      content: { data: Record<string, string> };
    };
  };
}

/** The real `expo-notifications`-backed implementation (Tier-3 on device). */
export function defaultNotificationsApi(): NotificationsApi {
  return {
    getPermissionsAsync: () =>
      Notifications.getPermissionsAsync().then((s) => ({ granted: s.granted })),
    requestPermissionsAsync: () =>
      Notifications.requestPermissionsAsync().then((s) => ({ granted: s.granted })),
    getExpoPushTokenAsync: (opts) =>
      Notifications.getExpoPushTokenAsync(opts as never).then((t) => ({ data: t.data })),
    setNotificationCategoryAsync: (identifier, actions) =>
      Notifications.setNotificationCategoryAsync(identifier, actions as never),
    addNotificationResponseReceivedListener: (listener) =>
      Notifications.addNotificationResponseReceivedListener(
        listener as never,
      ) as unknown as PushSubscription,
    DEFAULT_ACTION_IDENTIFIER: Notifications.DEFAULT_ACTION_IDENTIFIER,
  };
}

/**
 * Parse a notification's `content.data` into a typed [`IdOnlyPayload`], or `null`
 * if it is not the frozen D6 shape (defensive — a malformed/foreign push must not
 * crash the wakeup handler). All three fields are required.
 */
export function parseIdOnlyPayload(data: Record<string, unknown>): IdOnlyPayload | null {
  const notification_id = typeof data.notification_id === "string" ? data.notification_id : "";
  const kind = typeof data.kind === "string" ? data.kind : "";
  const source = typeof data.source === "string" ? data.source : "";
  if (!notification_id) return null;
  return { notification_id, kind, source };
}

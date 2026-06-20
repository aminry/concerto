// Post-wakeup fetch + lock-screen chip dispatch (Task 516; design/14 §3.3 / §6.3,
// design/16 §3.12). The FROZEN D6 push payload carries only an id (+ kind +
// source) — never the body — so on a wakeup the device FETCHES the full
// notification over the E2EE channel:
//
//   - `handleWakeup(payload)` → `Notifications.GetNotification({ id, device_id })`
//     (records the per-device `fetched_at`), returning the full `Notification`.
//   - `dispatchChip(...)` maps a lock-screen action chip (Approve/Dismiss, or any
//     `rule_id`) to `Notifications.ActOnChip({ notification_id, chip_id,
//     device_id })`. The Approve path is guarded by the BIOMETRIC GATE — you must
//     unlock before a tool approval is dispatched (design/16 §3.12).
//   - `handleNotificationResponse(response)` ties the OS callback together: read
//     the id-only payload + the tapped chip id, gate, then dispatch.
//
// Everything is INJECTED (DataClient + biometric api) so this is a pure Tier-2
// unit; the real native push UI + a live Core are Tier-3.
import { createClient } from "@connectrpc/connect";
import type { DataClient } from "@concerto/client";
import type { Notification } from "@concerto/client/gen/concerto/v1/notifications_pb";
import { Notifications } from "@concerto/client/gen/concerto/v1/notifications_pb";

import {
  type BiometricApi,
  runBiometricGate,
} from "./biometric-gate";
import {
  type IdOnlyPayload,
  type NotificationResponseLike,
  parseIdOnlyPayload,
} from "./expo-notifications";
import { APPROVE_ACTION_ID } from "./register";

/** Options shared by the wakeup actions. */
export interface WakeupContext {
  /** The active Core's DataClient (injected; mock in tests). */
  client: DataClient;
  /** This device's id (hex) — the per-Core `deviceIdHex`. */
  deviceId: string;
}

/**
 * The POST-WAKEUP FETCH: given the id-only payload, fetch the full notification
 * over the E2EE channel. Records `fetched_at` on the Core (the `device_id` field).
 * Returns `null` if the payload is not the frozen D6 shape.
 */
export async function handleWakeup(
  ctx: WakeupContext,
  payload: IdOnlyPayload | Record<string, string>,
): Promise<Notification | null> {
  const parsed = parseIdOnlyPayload(payload as Record<string, unknown>);
  if (!parsed) return null;

  const notifications = createClient(Notifications, ctx.client.transport);
  return notifications.getNotification({
    id: parsed.notification_id,
    deviceId: ctx.deviceId,
  });
}

/** The result of dispatching a chip. */
export type ChipDispatchResult =
  | {
      dispatched: true;
      /** "resolve_approval" | "send_message" | "navigate" (design/14 §6.3). */
      dispatchKind: string;
      dispatchArg: string;
      /** True iff this device lost the first-wins race. */
      alreadyResolved: boolean;
    }
  | { dispatched: false; reason: "biometric-blocked" };

/** Options for [`dispatchChip`]. */
export interface DispatchChipOptions extends WakeupContext {
  /** The affected notification id. */
  notificationId: string;
  /** The chip's `rule_id` (or the Approve/Dismiss action id). */
  chipId: string;
  /**
   * If true, run the biometric gate before dispatch (sensitive actions like an
   * approval). The biometric seam is injected via `biometricApi`.
   */
  requireBiometric?: boolean;
  /** The biometric seam (injected; mock in tests). */
  biometricApi?: BiometricApi;
}

/**
 * Dispatch a lock-screen / in-app action chip to `Notifications.ActOnChip`. When
 * `requireBiometric` is set, the BIOMETRIC GATE must pass first (fail-closed) —
 * otherwise the chip is NOT dispatched and `{ dispatched: false }` is returned.
 */
export async function dispatchChip(opts: DispatchChipOptions): Promise<ChipDispatchResult> {
  if (opts.requireBiometric) {
    const gate = await runBiometricGate({
      ...(opts.biometricApi ? { api: opts.biometricApi } : {}),
      promptMessage: "Confirm to approve",
      whenNotEnrolled: "block",
    });
    if (!gate.allowed) {
      return { dispatched: false, reason: "biometric-blocked" };
    }
  }

  const notifications = createClient(Notifications, opts.client.transport);
  const res = await notifications.actOnChip({
    notificationId: opts.notificationId,
    chipId: opts.chipId,
    deviceId: opts.deviceId,
  });
  return {
    dispatched: true,
    dispatchKind: res.dispatchKind,
    dispatchArg: res.dispatchArg,
    alreadyResolved: res.alreadyResolved,
  };
}

/**
 * Map an OS notification-response (a body tap or a lock-screen chip) to an action:
 *
 *   - a tap on the Approve chip → biometric-gated `ActOnChip(approve)`,
 *   - a tap on the Dismiss chip → ungated `ActOnChip(dismiss)`,
 *   - a body tap (`DEFAULT_ACTION_IDENTIFIER`) → biometric-gated POST-WAKEUP FETCH
 *     (opening the app from a wakeup is itself a sensitive action; design/16 §3.12),
 *   - any other `actionIdentifier` (a custom chip `rule_id`) → ungated `ActOnChip`.
 *
 * `defaultActionIdentifier` is injected (it's the module constant in production)
 * so the mapping is testable without the native module.
 */
export interface HandleResponseOptions extends WakeupContext {
  /** The OS action id sent for a body tap (the module's DEFAULT_ACTION_IDENTIFIER). */
  defaultActionIdentifier: string;
  /** The biometric seam (injected; mock in tests). */
  biometricApi?: BiometricApi;
}

/** What `handleNotificationResponse` did. */
export type ResponseOutcome =
  | { kind: "ignored" }
  | { kind: "opened"; notification: Notification | null }
  | { kind: "blocked" }
  | { kind: "chip"; result: ChipDispatchResult };

export async function handleNotificationResponse(
  opts: HandleResponseOptions,
  response: NotificationResponseLike,
): Promise<ResponseOutcome> {
  const payload = parseIdOnlyPayload(response.notification.request.content.data);
  if (!payload) return { kind: "ignored" };

  const action = response.actionIdentifier;

  // Body tap → open the app from the wakeup: biometric-gate, then fetch.
  if (action === opts.defaultActionIdentifier) {
    const gate = await runBiometricGate({
      ...(opts.biometricApi ? { api: opts.biometricApi } : {}),
      promptMessage: "Unlock to open Concerto",
      whenNotEnrolled: "allow", // opening the app is low-stakes vs. approving
    });
    if (!gate.allowed) return { kind: "blocked" };
    const notification = await handleWakeup(
      { client: opts.client, deviceId: opts.deviceId },
      payload,
    );
    return { kind: "opened", notification };
  }

  // A lock-screen chip — Approve is sensitive (gated); Dismiss / custom rule ids
  // are not. `action` is the chip's id (APPROVE_ACTION_ID / DISMISS_ACTION_ID / a
  // rule_id) and is forwarded as the `chip_id`.
  const requireBiometric = action === APPROVE_ACTION_ID;
  const result = await dispatchChip({
    client: opts.client,
    deviceId: opts.deviceId,
    notificationId: payload.notification_id,
    chipId: action,
    requireBiometric,
    ...(opts.biometricApi ? { biometricApi: opts.biometricApi } : {}),
  });
  return { kind: "chip", result };
}

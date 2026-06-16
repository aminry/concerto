// Post-wakeup fetch + chip dispatch + biometric gate tests (Task 516, Tier-2):
//   - an ID-only wakeup triggers GetNotification (the FROZEN D6 post-wakeup fetch),
//   - a malformed payload yields null (no RPC),
//   - dispatchChip -> ActOnChip; biometric gate ALLOWS (success) / BLOCKS (fail),
//   - handleNotificationResponse maps body-tap -> gated fetch, Approve chip ->
//     gated ActOnChip, Dismiss chip -> ungated ActOnChip.
import type { BiometricApi } from "./biometric-gate";
import type { NotificationResponseLike } from "./expo-notifications";
import { createMockPushDataClient } from "./test-data-client";
import { APPROVE_ACTION_ID, DISMISS_ACTION_ID } from "./register";
import {
  dispatchChip,
  handleNotificationResponse,
  handleWakeup,
} from "./wakeup";

const DEFAULT_ACTION = "expo.modules.notifications.actions.DEFAULT";

/** A biometric seam fake: `allow` ⇒ enrolled + auth success; `deny` ⇒ auth fail. */
function fakeBiometric(mode: "allow" | "deny" | "not-enrolled"): BiometricApi {
  return {
    hasHardwareAsync: jest.fn(async () => mode !== "not-enrolled"),
    isEnrolledAsync: jest.fn(async () => mode !== "not-enrolled"),
    authenticateAsync: jest.fn(async () => ({ success: mode === "allow" })),
  };
}

/** Build an OS notification response carrying the id-only payload. */
function response(actionIdentifier: string, data: Record<string, string>): NotificationResponseLike {
  return {
    actionIdentifier,
    notification: { request: { identifier: "req-1", content: { data } } },
  };
}

const ID_ONLY = { notification_id: "ntf_123", kind: "tool_approval_needed", source: "core-a" };

describe("handleWakeup (post-wakeup fetch)", () => {
  it("fetches the full notification via GetNotification with the device id", async () => {
    const dc = createMockPushDataClient({ notification: { id: "ntf_123", title: "Approve curl?" } });
    const n = await handleWakeup({ client: dc.client, deviceId: "dev-1" }, ID_ONLY);

    expect(n).not.toBeNull();
    expect(n!.id).toBe("ntf_123");
    expect(n!.title).toBe("Approve curl?");
    expect(dc.getNotificationCalls).toEqual([{ id: "ntf_123", deviceId: "dev-1" }]);
  });

  it("returns null (no RPC) on a payload missing notification_id", async () => {
    const dc = createMockPushDataClient();
    const n = await handleWakeup({ client: dc.client, deviceId: "dev-1" }, {
      kind: "x",
      source: "y",
    });
    expect(n).toBeNull();
    expect(dc.getNotificationCalls).toHaveLength(0);
  });
});

describe("dispatchChip (ActOnChip + biometric gate)", () => {
  it("dispatches an ungated chip to ActOnChip", async () => {
    const dc = createMockPushDataClient({ actOnChip: { dispatchKind: "navigate", dispatchArg: "wa/1" } });
    const r = await dispatchChip({
      client: dc.client,
      deviceId: "dev-1",
      notificationId: "ntf_123",
      chipId: DISMISS_ACTION_ID,
    });
    expect(r).toEqual({
      dispatched: true,
      dispatchKind: "navigate",
      dispatchArg: "wa/1",
      alreadyResolved: false,
    });
    expect(dc.actOnChipCalls).toEqual([
      { notificationId: "ntf_123", chipId: DISMISS_ACTION_ID, deviceId: "dev-1" },
    ]);
  });

  it("ALLOWS a biometric-gated chip when auth succeeds", async () => {
    const dc = createMockPushDataClient({ actOnChip: { dispatchKind: "resolve_approval" } });
    const r = await dispatchChip({
      client: dc.client,
      deviceId: "dev-1",
      notificationId: "ntf_123",
      chipId: APPROVE_ACTION_ID,
      requireBiometric: true,
      biometricApi: fakeBiometric("allow"),
    });
    expect(r.dispatched).toBe(true);
    expect(dc.actOnChipCalls).toHaveLength(1);
  });

  it("BLOCKS a biometric-gated chip when auth fails (NO ActOnChip)", async () => {
    const dc = createMockPushDataClient();
    const r = await dispatchChip({
      client: dc.client,
      deviceId: "dev-1",
      notificationId: "ntf_123",
      chipId: APPROVE_ACTION_ID,
      requireBiometric: true,
      biometricApi: fakeBiometric("deny"),
    });
    expect(r).toEqual({ dispatched: false, reason: "biometric-blocked" });
    expect(dc.actOnChipCalls).toHaveLength(0);
  });

  it("BLOCKS a biometric-gated chip when no auth is enrolled (fail-closed)", async () => {
    const dc = createMockPushDataClient();
    const r = await dispatchChip({
      client: dc.client,
      deviceId: "dev-1",
      notificationId: "ntf_123",
      chipId: APPROVE_ACTION_ID,
      requireBiometric: true,
      biometricApi: fakeBiometric("not-enrolled"),
    });
    expect(r.dispatched).toBe(false);
    expect(dc.actOnChipCalls).toHaveLength(0);
  });
});

describe("handleNotificationResponse (OS callback mapping)", () => {
  it("body tap -> biometric-gated post-wakeup fetch (allowed)", async () => {
    const dc = createMockPushDataClient({ notification: { id: "ntf_123" } });
    const out = await handleNotificationResponse(
      {
        client: dc.client,
        deviceId: "dev-1",
        defaultActionIdentifier: DEFAULT_ACTION,
        biometricApi: fakeBiometric("allow"),
      },
      response(DEFAULT_ACTION, ID_ONLY),
    );
    expect(out.kind).toBe("opened");
    if (out.kind === "opened") expect(out.notification?.id).toBe("ntf_123");
    expect(dc.getNotificationCalls).toHaveLength(1);
  });

  it("Approve chip -> biometric-gated ActOnChip (blocked on auth fail)", async () => {
    const dc = createMockPushDataClient();
    const out = await handleNotificationResponse(
      {
        client: dc.client,
        deviceId: "dev-1",
        defaultActionIdentifier: DEFAULT_ACTION,
        biometricApi: fakeBiometric("deny"),
      },
      response(APPROVE_ACTION_ID, ID_ONLY),
    );
    expect(out.kind).toBe("chip");
    if (out.kind === "chip") expect(out.result.dispatched).toBe(false);
    expect(dc.actOnChipCalls).toHaveLength(0);
  });

  it("Dismiss chip -> ungated ActOnChip (no biometric prompt)", async () => {
    const bio = fakeBiometric("deny"); // even a failing gate must not block dismiss
    const dc = createMockPushDataClient();
    const out = await handleNotificationResponse(
      {
        client: dc.client,
        deviceId: "dev-1",
        defaultActionIdentifier: DEFAULT_ACTION,
        biometricApi: bio,
      },
      response(DISMISS_ACTION_ID, ID_ONLY),
    );
    expect(out.kind).toBe("chip");
    if (out.kind === "chip") expect(out.result.dispatched).toBe(true);
    expect(bio.authenticateAsync).not.toHaveBeenCalled();
    expect(dc.actOnChipCalls).toHaveLength(1);
  });

  it("ignores a response whose payload is not the id-only shape", async () => {
    const dc = createMockPushDataClient();
    const out = await handleNotificationResponse(
      {
        client: dc.client,
        deviceId: "dev-1",
        defaultActionIdentifier: DEFAULT_ACTION,
        biometricApi: fakeBiometric("allow"),
      },
      response(DEFAULT_ACTION, { foo: "bar" }),
    );
    expect(out).toEqual({ kind: "ignored" });
    expect(dc.getNotificationCalls).toHaveLength(0);
  });
});

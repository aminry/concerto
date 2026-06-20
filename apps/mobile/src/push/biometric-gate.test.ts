// Biometric gate tests (Task 516, Tier-2). The gate is fail-closed for sensitive
// actions: enrolled + auth success ⇒ allowed; enrolled + auth fail ⇒ blocked; not
// enrolled ⇒ blocked by default, allowed only when `whenNotEnrolled: "allow"`.
import {
  type BiometricApi,
  requireBiometric,
  runBiometricGate,
} from "./biometric-gate";

function fake(over: Partial<BiometricApi> = {}): BiometricApi {
  return {
    hasHardwareAsync: jest.fn(async () => true),
    isEnrolledAsync: jest.fn(async () => true),
    authenticateAsync: jest.fn(async () => ({ success: true })),
    ...over,
  };
}

describe("runBiometricGate", () => {
  it("allows when enrolled and auth succeeds", async () => {
    const api = fake();
    const out = await runBiometricGate({ api });
    expect(out).toEqual({ allowed: true, via: "authenticated" });
    expect(api.authenticateAsync).toHaveBeenCalledTimes(1);
  });

  it("blocks when enrolled and auth fails", async () => {
    const api = fake({ authenticateAsync: jest.fn(async () => ({ success: false, error: "lockout" })) });
    const out = await runBiometricGate({ api });
    expect(out).toEqual({ allowed: false, reason: "failed" });
  });

  it("blocks when no auth is enrolled (default fail-closed)", async () => {
    const api = fake({ isEnrolledAsync: jest.fn(async () => false) });
    const out = await runBiometricGate({ api });
    expect(out).toEqual({ allowed: false, reason: "not-enrolled" });
    expect(api.authenticateAsync).not.toHaveBeenCalled();
  });

  it("blocks when there is no biometric hardware (default fail-closed)", async () => {
    const api = fake({ hasHardwareAsync: jest.fn(async () => false) });
    const out = await runBiometricGate({ api });
    expect(out).toEqual({ allowed: false, reason: "not-enrolled" });
  });

  it("allows a not-enrolled device when whenNotEnrolled is 'allow'", async () => {
    const api = fake({ isEnrolledAsync: jest.fn(async () => false) });
    const out = await runBiometricGate({ api, whenNotEnrolled: "allow" });
    expect(out).toEqual({ allowed: true, via: "no-enrollment-allowed" });
  });

  it("fails closed when authenticateAsync rejects (native throw)", async () => {
    const api = fake({
      authenticateAsync: jest.fn(async () => {
        throw new Error("OS interruption");
      }),
    });
    const out = await runBiometricGate({ api });
    expect(out).toEqual({ allowed: false, reason: "failed" });
  });

  it("fails closed when the probe rejects (native throw)", async () => {
    const api = fake({
      isEnrolledAsync: jest.fn(async () => {
        throw new Error("native module missing");
      }),
    });
    const out = await runBiometricGate({ api });
    expect(out).toEqual({ allowed: false, reason: "not-enrolled" });
    // A failed probe must not reach the auth prompt.
    expect(api.authenticateAsync).not.toHaveBeenCalled();
  });

  it("a rejecting probe stays fail-closed even with whenNotEnrolled='allow'", async () => {
    const api = fake({
      hasHardwareAsync: jest.fn(async () => {
        throw new Error("hardware query failed");
      }),
    });
    const out = await runBiometricGate({ api, whenNotEnrolled: "allow" });
    expect(out).toEqual({ allowed: false, reason: "not-enrolled" });
  });

  it("requireBiometric collapses to a boolean", async () => {
    expect(await requireBiometric({ api: fake() })).toBe(true);
    expect(
      await requireBiometric({ api: fake({ authenticateAsync: jest.fn(async () => ({ success: false })) }) }),
    ).toBe(false);
  });
});

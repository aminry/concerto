// The biometric gate (Task 516; design/16 §3.12 — guard sensitive actions:
// approving a chip, opening the app from a wakeup). A thin, INJECTABLE seam over
// `expo-local-authentication` (a NATIVE module — Face ID / Touch ID / device
// passcode is Tier-3, unavailable in jest).
//
// Policy (deliberately FAIL-CLOSED for sensitive actions): if the device HAS
// biometric/enrolled hardware, an `authenticate()` must succeed before the action
// runs; if the device has NO enrolled authentication at all, we do NOT silently
// allow a sensitive action — the caller decides (see `requireBiometric`). This
// keeps "approve a tool from a stolen, unlocked phone" from being a free action.
import * as LocalAuthentication from "expo-local-authentication";

/** The narrow `expo-local-authentication` surface the gate needs. */
export interface BiometricApi {
  /** Whether the device has biometric hardware. */
  hasHardwareAsync(): Promise<boolean>;
  /** Whether the user has enrolled a biometric / passcode. */
  isEnrolledAsync(): Promise<boolean>;
  /** Prompt for authentication; resolves `{ success }`. */
  authenticateAsync(opts?: {
    promptMessage?: string;
    cancelLabel?: string;
  }): Promise<{ success: boolean; error?: string }>;
}

/** The real `expo-local-authentication`-backed implementation (Tier-3). */
export function defaultBiometricApi(): BiometricApi {
  return {
    hasHardwareAsync: () => LocalAuthentication.hasHardwareAsync(),
    isEnrolledAsync: () => LocalAuthentication.isEnrolledAsync(),
    authenticateAsync: (opts) =>
      LocalAuthentication.authenticateAsync(opts).then((r) => ({
        success: r.success,
        ...(r.success ? {} : { error: (r as { error?: string }).error ?? "failed" }),
      })),
  };
}

/** Why the gate allowed / blocked an action. */
export type BiometricOutcome =
  | { allowed: true; via: "authenticated" | "no-enrollment-allowed" }
  | { allowed: false; reason: "failed" | "not-enrolled" };

/** Options for [`runBiometricGate`]. */
export interface BiometricGateOptions {
  /** The auth seam (defaults to the real module). */
  api?: BiometricApi;
  /** Prompt copy shown in the OS sheet. */
  promptMessage?: string;
  /**
   * What to do when the device has NO enrolled auth (no Face ID, no passcode).
   * Sensitive actions pass `"block"` (FAIL-CLOSED); low-stakes opens may pass
   * `"allow"` so a passcode-less device is not bricked. Default `"block"`.
   */
  whenNotEnrolled?: "allow" | "block";
}

/**
 * Run the biometric gate. Resolves a structured [`BiometricOutcome`] — it never
 * throws so callers can branch on `allowed`. When enrolled, requires a successful
 * `authenticateAsync`; when not enrolled, obeys `whenNotEnrolled` (default block).
 */
export async function runBiometricGate(
  opts: BiometricGateOptions = {},
): Promise<BiometricOutcome> {
  const api = opts.api ?? defaultBiometricApi();
  const whenNotEnrolled = opts.whenNotEnrolled ?? "block";

  // FAIL-CLOSED: the probe is a native call and may reject (degraded build,
  // missing module, system error). Treat any throw as "no usable enrollment".
  let hasHardware: boolean;
  let enrolled: boolean;
  try {
    [hasHardware, enrolled] = await Promise.all([
      api.hasHardwareAsync(),
      api.isEnrolledAsync(),
    ]);
  } catch {
    return { allowed: false, reason: "not-enrolled" };
  }

  if (!hasHardware || !enrolled) {
    return whenNotEnrolled === "allow"
      ? { allowed: true, via: "no-enrollment-allowed" }
      : { allowed: false, reason: "not-enrolled" };
  }

  // FAIL-CLOSED: a native auth call may reject (OS interruption, hardware
  // error). Treat any throw as a blocked sensitive action.
  let res: { success: boolean; error?: string };
  try {
    res = await api.authenticateAsync({
      promptMessage: opts.promptMessage ?? "Confirm it's you",
      cancelLabel: "Cancel",
    });
  } catch {
    return { allowed: false, reason: "failed" };
  }
  return res.success
    ? { allowed: true, via: "authenticated" }
    : { allowed: false, reason: "failed" };
}

/** Convenience boolean form: `true` iff the gate allowed the action. */
export async function requireBiometric(opts: BiometricGateOptions = {}): Promise<boolean> {
  return (await runBiometricGate(opts)).allowed;
}

// The pairing orchestration (Task 511; design/16 §3.8 step 3–6). Given a scanned
// QR string and the native module, this runs the full pair flow and persists the
// result to the multi-Core registry:
//
//   1. parse the connect blob (`connect-blob.ts`),
//   2. generate a fresh device keypair (`module.generateDeviceKeypair`),
//   3. run the Noise-XX handshake (`module.pair`) → signed device cert,
//   4. persist seed + cert + blob to secure-store and make the Core active
//      (`core-store.addCore`).
//
// Step 7 (register the Expo push token via `Notifications.UpdateDevicePushToken`)
// is a SEPARATE task (503/push wiring) and is intentionally NOT done here.
//
// All side-effecting collaborators (the native module) are injected so this is a
// pure Tier-2 unit under jest with `createMockConcertoIroh(...)` + the
// secure-store mock.
import type { ConcertoIrohModule } from "../native/ConcertoIroh";
import { parseConnectBlob } from "./connect-blob";
import { addCore, type StoredCore } from "./core-store";

/** Lowercase hex of a byte array (for the stored `device_id`). */
function toHex(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += bytes[i].toString(16).padStart(2, "0");
  return s;
}

/** Options for [`pairWithQr`]. */
export interface PairWithQrOptions {
  /** A human label for the Core (defaults to the endpoint id's short form). */
  coreLabel?: string;
  /** The device name recorded in the cert (defaults to "Concerto Mobile"). */
  deviceName?: string;
}

/** The result of a successful pairing. */
export interface PairResult {
  /** The newly-paired (and now active) Core's index entry. */
  core: StoredCore;
}

/**
 * Pair this device with a Core from a scanned QR string. Throws
 * `ConnectBlobParseError` on a bad QR, or the native module's error on a failed
 * handshake. On success the new Core is persisted and becomes active.
 */
export async function pairWithQr(
  module: ConcertoIrohModule,
  qr: string,
  opts: PairWithQrOptions = {},
): Promise<PairResult> {
  const { blob, pairingToken } = parseConnectBlob(qr);

  const keypair = await module.generateDeviceKeypair();
  const deviceName = opts.deviceName ?? "Concerto Mobile";

  const signedCert = await module.pair(
    { blob, pairingToken, deviceName },
    keypair.seed,
  );

  const id = blob.endpointId;
  const core = await addCore({
    id,
    label: opts.coreLabel ?? defaultLabel(id),
    blob,
    deviceIdHex: toHex(keypair.deviceId),
    deviceSeed: keypair.seed,
    signedCert,
  });

  return { core };
}

/** A short, human-ish default label from the endpoint id (first 8 chars). */
function defaultLabel(endpointId: string): string {
  return `Core ${endpointId.slice(0, 8)}`;
}

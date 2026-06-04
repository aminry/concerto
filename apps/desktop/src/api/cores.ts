// Typed binding over the connected-Core registry read-commands (Task 218,
// `design/15 §3.10.1`).
//
// The Rust shell owns the registry (cores.json + keychain); the renderer is
// forbidden from touching gRPC / keychain / fs directly (Tauri capabilities).
// This module is the thin typed surface Task 219's pairing UI + the
// Connect-to-Core picker (Task 601) read. **FROZEN binding surface** — the
// return types are the contract those tasks consume. Server-canonical registry
// data is fetched here and cached in React Query; the UI-only active-Core
// *selection* lives in the Zustand slice (`src/state/useCoresStore.ts`).
//
// Secrets (device cert, device private key) are NEVER exposed to the renderer —
// they live in the OS keychain keyed by `core_id` and stay Rust-side.

import { invoke } from "@tauri-apps/api/core";

/// The wire string for a paired Core's transport, as the Rust
/// `cores_registry::TransportKind` serializes it (serde `rename_all =
/// "lowercase"`). This is the registry's *stored* transport; it agrees with the
/// per-connection `ServerCapabilities.transport_kind` (Task 201, numeric proto
/// enum in `runtime.ts`): `"uds"` ↔ `TransportKind.Uds`, `"iroh"` ↔
/// `TransportKind.Iroh`. **FROZEN.**
export type CoreTransportKind = "uds" | "iroh";

/// A paired Core as the renderer sees it — cleartext metadata only (`design/15
/// §3.10.1`). Mirrors the Rust `PairedCoreView`. No secrets. **FROZEN shape**
/// (new fields append-only).
export type PairedCore = {
  /// `BLAKE2b(core_pubkey)` lowercase hex — the registry key.
  core_id: string;
  /// User-friendly name ("This machine", "Home workstation", "Cloud VM").
  display_name: string;
  /// The transport this Core is reached over.
  transport_kind: CoreTransportKind;
  /// The Iroh endpoint id (split-host only); `null` for UDS.
  iroh_endpoint_id: string | null;
  /// Last successful connection (unix epoch seconds), or `null`.
  last_connected_at: number | null;
  /// Whether this Core is the currently active one.
  is_active: boolean;
};

/// List every paired Core (cleartext metadata). The Connect-to-Core picker
/// (Task 219/601) and Settings → Connected Cores read this.
export async function listPairedCores(): Promise<PairedCore[]> {
  return invoke<PairedCore[]>("list_paired_cores");
}

/// The active Core (or `null` when none is set). Carries its `transport_kind`
/// so the renderer can branch remote-mode affordances (Task 602).
export async function getActiveCore(): Promise<PairedCore | null> {
  return invoke<PairedCore | null>("get_active_core");
}

/// Persist the active-Core pointer (server-canonical). The full
/// disconnect/reconnect switch UX is Task 601; this is the registry-write seam
/// it calls. The UI-only selection is mirrored into the Zustand slice.
export async function setActiveCore(coreId: string): Promise<void> {
  await invoke<void>("set_active_core", { coreId });
}

// ---------------------------------------------------------------------------
// Pairing command bindings (Task 219).
//
// The renderer drives the split-host pairing ceremony — and the co-located
// "Reveal pairing QR" affordance — entirely through the Tauri commands below.
// The ceremony itself (Noise XX, `Devices.StartPairing`/`CompletePairing`/
// `RevokeDevice`, keychain writes, the `PairedCore` row) lives in the Rust
// shell (Tasks 207/209/218); the renderer never speaks gRPC. **FROZEN
// pairing-UI ↔ Tauri-command contract** — the command names + arg/return
// shapes here are the contract the shell implements (co-designed with 218's
// `cores.ts` read seams). Mutating commands invalidate the React-Query
// `["cores"]` cache so the picker + Connected-Cores list re-fetch.
// ---------------------------------------------------------------------------

/// The decoded pairing payload a Core emits (`design/12 §3.3`): the renderer
/// renders this as a QR for the co-located "Reveal pairing QR" affordance, and
/// it is the shape `concerto pair` (Task 713) prints base64-encoded for the
/// "Paste token" path. **FROZEN envelope.** `core_pubkey` and `pairing_token`
/// are base64 (standard, no-pad-agnostic) byte strings; exactly one of
/// `lan_endpoint` / `iroh_endpoint_id` carries the LAN-vs-relay path, with
/// `relay_hint` the fallback. New fields are append-only.
export type PairingPayload = {
  /// The Core's Ed25519 identity public key (base64), for cross-machine
  /// validation of the issued cert.
  core_pubkey: string;
  /// The 32-byte one-shot pairing token (base64). 60s TTL, one-shot
  /// (`design/12 §3.3`).
  pairing_token: string;
  /// The mDNS-resolved LAN endpoint, when discovered (LAN-direct path).
  lan_endpoint?: string | null;
  /// The Iroh endpoint id (relay/hole-punch path). `concerto pair` emits this
  /// for headless Cores; the LAN path may omit it.
  iroh_endpoint_id?: string | null;
  /// Relay hint used when the LAN path is unavailable (`design/12 §3.3` R-3).
  relay_hint?: string | null;
};

/// Encode the canonical base64 JSON envelope back into the string a `concerto
/// pair` would print / a QR would carry. Used to render the local Core's QR
/// from a decoded `PairingPayload` and (in tests) to build fixture tokens.
/// Mirrors the shell's `base64(json)` encoding (`design/12 §3.3`).
export function encodePairingPayload(payload: PairingPayload): string {
  const json = JSON.stringify(payload);
  // `btoa` over the UTF-8 bytes (the JSON here is ASCII-only field names +
  // base64 values, so `btoa(json)` is safe).
  return btoa(json);
}

/// Decode the base64 token a user pastes (or a QR yields) into the typed
/// `PairingPayload` envelope. Throws a human-readable `Error` when the string
/// is not valid base64-JSON or is missing the two required fields — the
/// "Paste token" path surfaces the message inline. This is the renderer-side
/// validation; the shell re-validates the token (TTL, one-shot) authoritatively.
export function decodePairingPayload(token: string): PairingPayload {
  const trimmed = token.trim();
  if (trimmed.length === 0) {
    throw new Error("Paste the pairing token from `concerto pair`.");
  }
  let json: string;
  try {
    json = atob(trimmed);
  } catch {
    throw new Error("That doesn't look like a pairing token (invalid base64).");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new Error("That doesn't look like a pairing token (not JSON).");
  }
  if (typeof parsed !== "object" || parsed === null) {
    throw new Error("That doesn't look like a pairing token.");
  }
  const obj = parsed as Record<string, unknown>;
  if (typeof obj.core_pubkey !== "string" || obj.core_pubkey.length === 0) {
    throw new Error("Pairing token is missing the Core public key.");
  }
  if (typeof obj.pairing_token !== "string" || obj.pairing_token.length === 0) {
    throw new Error("Pairing token is missing the pairing secret.");
  }
  return {
    core_pubkey: obj.core_pubkey,
    pairing_token: obj.pairing_token,
    lan_endpoint:
      typeof obj.lan_endpoint === "string" ? obj.lan_endpoint : null,
    iroh_endpoint_id:
      typeof obj.iroh_endpoint_id === "string" ? obj.iroh_endpoint_id : null,
    relay_hint: typeof obj.relay_hint === "string" ? obj.relay_hint : null,
  };
}

/// Ask the local (co-located) Core to start a pairing and return the payload to
/// render as a QR (`Devices.StartPairing` + `GetCoreInfo` in the shell). Only
/// valid when the active Core's `transport_kind === Uds` — the renderer gates
/// the "Reveal pairing QR" entry point on that (`design/15 §3.11`). The token
/// inside has a **60s TTL** (`design/12 §3.3`); the shell returns it fresh on
/// each call so the UI can show a countdown and re-request on expiry.
///
/// **OWED (Rust):** 218 froze the read commands; the pairing *write* commands
/// (`start_pairing_show`, `complete_pairing_from_payload`,
/// `rename_paired_core`, `remove_paired_core`) are the shell implementations a
/// follow-up (Task 601 / a Rust pairing-commands task) owes — see this task's
/// Handoff. The renderer calls the frozen names; tests mock `invoke`.
export async function startPairingShow(): Promise<PairingPayload> {
  return invoke<PairingPayload>("start_pairing_show");
}

/// The result of a completed pairing: the new Core's `core_id`
/// (`BLAKE2b(core_pubkey)` hex) and a suggested display name (the Core's
/// hostname, from `GetCoreInfo`) the name-the-pairing step pre-fills.
export type CompletePairingResult = {
  core_id: string;
  /// Default name suggestion (the Core machine's hostname). The user can edit
  /// it before it is persisted via [`renamePairedCore`].
  suggested_name: string;
};

/// Drive the split-host pairing ceremony from a pasted/scanned token. The shell
/// decodes the envelope, generates the Desktop's Ed25519 keypair, runs the
/// Noise XX handshake bootstrapped by `pairing_token`, calls
/// `Devices.CompletePairing`, stores the device key + cert in the keychain, and
/// writes the `PairedCore` row (`design/15 §3.10.3`). Returns the new `core_id`
/// + suggested name; the caller then renames + sets it active. A rejected
/// `invoke` surfaces the `{kind,message}` envelope (e.g. an **expired token**
/// → `Rpc: pairing token expired`).
export async function completePairingFromPayload(
  token: string,
): Promise<CompletePairingResult> {
  return invoke<CompletePairingResult>("complete_pairing_from_payload", {
    token,
  });
}

/// Rename a paired Core (the name-the-pairing step + the Connected-Cores
/// "Rename" row). Persists to `cores.json` (server-canonical); the caller
/// invalidates `["cores"]`.
export async function renamePairedCore(
  coreId: string,
  displayName: string,
): Promise<void> {
  await invoke<void>("rename_paired_core", { coreId, displayName });
}

/// Remove a pairing: deletes the local `cores.json` row + its keychain secrets
/// and best-effort calls `Devices.RevokeDevice` on the Core (`design/15
/// §3.10.4`; the Core's revocation list is authoritative). The caller
/// invalidates `["cores"]`.
export async function removePairedCore(coreId: string): Promise<void> {
  await invoke<void>("remove_paired_core", { coreId });
}

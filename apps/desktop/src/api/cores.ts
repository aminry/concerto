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

// Connect-to-Core picker (`design/15 §3.10.2` step 4).
//
// Lists the Cores this Desktop has previously paired with (from 218's registry
// binding, with reachability status dots) and the two entry points:
//   - "Start a local Core" — delegates to the shell's auto-spawn command that
//     Task 601 fleshes out; here the button is wired but the action is a thin
//     stub behind a frozen command name (601 supplies the diagnostics/retry).
//   - "Pair with a remote Core" — opens the pairing flow (`PairCoreModal`).
//
// This is the picker UI only. The first-launch auto-spawn decision tree (steps
// 0–3) and the full disconnect/reconnect switch orchestration are Task 601;
// this component renders the surface 601 drives.

import { useQuery, useQueryClient } from "@tanstack/react-query";

import {
  listPairedCores,
  setActiveCore,
  type PairedCore,
} from "../api/cores";
import { formatError } from "../api/errors";
import { useUiStore } from "../state/useUiStore";
import { useCoresStore } from "../state/useCoresStore";
import { Button } from "./ui/button";
import { StatusDot, type DotStatus } from "./ui/status-dot";

/// Map a paired Core's reachability to a status-dot semantic
/// (`design/15 §3.10.4`: reachable / unreachable / never-connected).
export function coreStatus(core: PairedCore): DotStatus {
  if (core.is_active) return "ok";
  if (core.last_connected_at == null) return "idle"; // never connected
  return "warning"; // previously connected, currently unverified/unreachable
}

export function ConnectCorePicker(): JSX.Element | null {
  const open = useUiStore((s) => s.connectCoreOpen);
  const setOpen = useUiStore((s) => s.setConnectCoreOpen);
  const setPairingOpen = useUiStore((s) => s.setPairingOpen);
  const setPendingActiveCore = useCoresStore((s) => s.setPendingActiveCore);
  const queryClient = useQueryClient();

  const coresQuery = useQuery({
    queryKey: ["cores"],
    queryFn: listPairedCores,
    enabled: open,
  });

  if (!open) return null;

  async function connect(core: PairedCore): Promise<void> {
    // Optimistic highlight while the switch commits (Task 601 owns the full
    // teardown/reload; here we persist the pointer + refetch).
    setPendingActiveCore(core.core_id);
    try {
      await setActiveCore(core.core_id);
      await queryClient.invalidateQueries({ queryKey: ["cores"] });
      setOpen(false);
    } finally {
      setPendingActiveCore(null);
    }
  }

  const cores = coresQuery.data ?? [];

  return (
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      onClick={() => setOpen(false)}
      role="presentation"
    >
      <div
        className="w-[26rem] max-w-[90vw] rounded-lg border border-border bg-surface p-5 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="Connect to a Core"
      >
        <h2 className="mb-1 text-sm font-semibold text-foreground">
          Connect to a Core
        </h2>
        <p className="mb-4 text-xs text-muted">
          Choose a Core to drive your sessions, or pair a new one.
        </p>

        <div className="mb-4 space-y-1">
          {coresQuery.isLoading && (
            <p className="text-xs text-faint">Loading paired Cores…</p>
          )}
          {coresQuery.isError && (
            <p className="text-xs text-err">
              {formatError(coresQuery.error)}
            </p>
          )}
          {coresQuery.isSuccess && cores.length === 0 && (
            <p className="text-xs text-faint">
              No paired Cores yet. Start a local Core or pair a remote one.
            </p>
          )}
          {cores.map((core) => (
            <button
              key={core.core_id}
              type="button"
              onClick={() => void connect(core)}
              className="flex w-full items-center gap-2 rounded-md border border-border px-3 py-2 text-left transition-colors hover:bg-surface-2"
            >
              <StatusDot status={coreStatus(core)} />
              <span className="flex-1 truncate text-sm text-foreground">
                {core.display_name}
              </span>
              <span className="text-[11px] text-faint">
                {core.transport_kind === "uds" ? "Local" : "Remote"}
              </span>
              <span className="text-xs text-accent">Connect</span>
            </button>
          ))}
        </div>

        <div className="flex flex-col gap-2 border-t border-border pt-4">
          <Button variant="outline" onClick={() => void startLocalCore()}>
            Start a local Core
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              setOpen(false);
              setPairingOpen(true);
            }}
          >
            Pair with a remote Core
          </Button>
        </div>
      </div>
    </div>
  );
}

/// "Start a local Core" — wired here, fleshed out by Task 601. It calls the
/// shell's auto-spawn command (frozen name `start_local_core`); 601 supplies
/// the diagnostics, retry-poll, and embedded-mode branch (`design/15 §3.10.2`
/// step 3). The picker only needs to fire it; the launch banner / retry UX is
/// 601's. The command resolves once the local UDS is reachable (or rejects).
async function startLocalCore(): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("start_local_core");
}

// Settings → Connected Cores list (`design/15 §3.10.4`).
//
// Lists every `PairedCore` with a reachable / unreachable / never-connected
// status dot and per-row actions: Switch active, Rename, Remove pairing, plus
// an "Add another" entry that re-enters the pairing flow.
//
//   - Switch active → `set_active_core` then a renderer reload so cached state
//     from the previous Core never lingers (`design/15 §3.10.4`). The reload is
//     guarded behind a window check so it's inert (and assertable) in tests.
//   - Rename → inline edit → `rename_paired_core`, invalidate `["cores"]`.
//   - Remove → `remove_paired_core` (best-effort `Devices.RevokeDevice` in the
//     shell), invalidate `["cores"]`.
//
// Server-canonical data (the list + reachability) comes from React Query keyed
// on `["cores"]`; mutations invalidate it. The `set_active_core` write reuses
// 218's frozen read/set seams; the pairing writes are 219's frozen commands.

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  listPairedCores,
  removePairedCore,
  renamePairedCore,
  setActiveCore,
  type PairedCore,
} from "../api/cores";
import { formatError } from "../api/errors";
import { useUiStore } from "../state/useUiStore";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { coreStatus } from "./ConnectCorePicker";
import { StatusDot } from "./ui/status-dot";

/// Clear cached renderer state after switching the active Core
/// (`design/15 §3.10.4`). A full reload is the simplest correct way to drop all
/// React-Query caches + component state tied to the previous Core. Guarded so
/// tests (no real `location.reload`) stay deterministic.
function reloadRenderer(): void {
  if (typeof window !== "undefined" && typeof window.location?.reload === "function") {
    window.location.reload();
  }
}

export function ConnectedCoresList(): JSX.Element {
  const setPairingOpen = useUiStore((s) => s.setPairingOpen);
  const queryClient = useQueryClient();
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const coresQuery = useQuery({
    queryKey: ["cores"],
    queryFn: listPairedCores,
  });

  const switchMutation = useMutation({
    mutationFn: async (coreId: string) => {
      await setActiveCore(coreId);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["cores"] });
      // Drop all cached state from the previous Core.
      reloadRenderer();
    },
    onError: (e) => setError(formatError(e)),
  });

  const renameMutation = useMutation({
    mutationFn: async (vars: { coreId: string; name: string }) => {
      await renamePairedCore(vars.coreId, vars.name);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["cores"] });
      setRenamingId(null);
      setRenameValue("");
    },
    onError: (e) => setError(formatError(e)),
  });

  const removeMutation = useMutation({
    mutationFn: async (coreId: string) => {
      await removePairedCore(coreId);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["cores"] });
    },
    onError: (e) => setError(formatError(e)),
  });

  const cores = coresQuery.data ?? [];

  function startRename(core: PairedCore): void {
    setError(null);
    setRenamingId(core.core_id);
    setRenameValue(core.display_name);
  }

  return (
    <section className="space-y-3">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-semibold uppercase tracking-wider text-muted">
          Connected Cores
        </h3>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => setPairingOpen(true)}
        >
          Add another
        </Button>
      </div>

      {coresQuery.isLoading && <p className="text-xs text-faint">Loading…</p>}
      {coresQuery.isSuccess && cores.length === 0 && (
        <p className="text-xs text-faint">No paired Cores yet.</p>
      )}
      {error && <p className="text-xs text-err">{error}</p>}

      <ul className="space-y-2">
        {cores.map((core) => {
          const isRenaming = renamingId === core.core_id;
          return (
            <li
              key={core.core_id}
              className="rounded-md border border-border px-3 py-2"
            >
              <div className="flex items-center gap-2">
                <StatusDot status={coreStatus(core)} />
                {isRenaming ? (
                  <Input
                    value={renameValue}
                    onChange={(e) => setRenameValue(e.target.value)}
                    aria-label={`Rename ${core.display_name}`}
                    autoFocus
                    className="flex-1"
                  />
                ) : (
                  <span className="flex-1 truncate text-sm text-foreground">
                    {core.display_name}
                    {core.is_active && (
                      <span className="ml-2 text-[11px] text-ok">active</span>
                    )}
                  </span>
                )}
                <span className="text-[11px] text-faint">
                  {core.transport_kind === "uds" ? "Local" : "Remote"}
                </span>
              </div>

              <div className="mt-2 flex flex-wrap gap-2">
                {isRenaming ? (
                  <>
                    <Button
                      size="sm"
                      variant="primary"
                      disabled={
                        renameMutation.isPending || !renameValue.trim()
                      }
                      onClick={() =>
                        renameMutation.mutate({
                          coreId: core.core_id,
                          name: renameValue.trim(),
                        })
                      }
                    >
                      Save
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        setRenamingId(null);
                        setRenameValue("");
                      }}
                    >
                      Cancel
                    </Button>
                  </>
                ) : (
                  <>
                    {!core.is_active && (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={switchMutation.isPending}
                        onClick={() => switchMutation.mutate(core.core_id)}
                      >
                        Switch active
                      </Button>
                    )}
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => startRename(core)}
                    >
                      Rename
                    </Button>
                    <Button
                      size="sm"
                      variant="danger"
                      disabled={removeMutation.isPending}
                      onClick={() => removeMutation.mutate(core.core_id)}
                    >
                      Remove
                    </Button>
                  </>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

// Settings panel — hosts the Add Repository form and (Task 219) the
// Connected Cores section.
//
// Rendered as a right-side overlay when `useUiStore.settingsOpen` is true. The
// Connected Cores section lists every paired Core with switch/rename/remove
// rows + "Add another", and exposes the UDS-gated "Reveal pairing QR"
// affordance (`design/15 §3.10.4` / §3.11). The QR-show is conditional on the
// active Core's `transport_kind === Uds`; for a remote (Iroh) active Core it
// renders the "use the Core machine's tray or `concerto pair`" hint instead.

import { useQuery } from "@tanstack/react-query";

import { getActiveCore } from "../api/cores";
import { TransportKind } from "../api/runtime";
import { useUiStore } from "../state/useUiStore";
import { AddRepositoryForm } from "./AddRepositoryForm";
import { ConnectedCoresList } from "./ConnectedCoresList";
import { ShowPairingQr } from "./ShowPairingQr";
import { Button } from "./ui/button";

/// Map the registry's stored transport string to the numeric `TransportKind`
/// the QR-show gate keys off. The active Core's transport_kind is "uds" |
/// "iroh" (cleartext registry metadata); a UDS active Core can reveal a local
/// pairing QR, a remote one cannot (`design/15 §3.11`).
function transportKindOf(stored: "uds" | "iroh" | undefined): TransportKind {
  return stored === "iroh" ? TransportKind.Iroh : TransportKind.Uds;
}

export function SettingsPanel(): JSX.Element | null {
  const open = useUiStore((s) => s.settingsOpen);
  const setOpen = useUiStore((s) => s.setSettingsOpen);

  const activeCoreQuery = useQuery({
    queryKey: ["cores", "active"],
    queryFn: getActiveCore,
    enabled: open,
  });

  if (!open) return null;

  const activeTransport = transportKindOf(
    activeCoreQuery.data?.transport_kind,
  );

  return (
    <div
      className="fixed inset-0 z-40 flex justify-end bg-black/40"
      onClick={() => setOpen(false)}
      role="presentation"
    >
      <aside
        className="w-[24rem] max-w-[90vw] h-full overflow-y-auto border-l border-border bg-background p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between mb-4">
          <h2 className="text-sm font-semibold uppercase tracking-wider text-muted">
            Settings
          </h2>
          <Button variant="ghost" onClick={() => setOpen(false)}>
            Close
          </Button>
        </header>

        <div className="space-y-6">
          <div className="space-y-3">
            <ConnectedCoresList />
            <div>
              <h4 className="mb-2 text-xs uppercase tracking-wider text-faint">
                Reveal pairing QR
              </h4>
              <ShowPairingQr transportKind={activeTransport} />
            </div>
          </div>

          <AddRepositoryForm />
        </div>
      </aside>
    </div>
  );
}

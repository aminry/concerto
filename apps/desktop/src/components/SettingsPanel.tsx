// Settings panel — V0.1 placeholder hosting the Add Repository form.
//
// Rendered as a right-side overlay when `useUiStore.settingsOpen` is
// true. Future tasks promote this to a full settings tree (per-project
// agents, permission defaults, MCP config); for now it's a single
// section.

import { useUiStore } from "../state/useUiStore";
import { AddRepositoryForm } from "./AddRepositoryForm";
import { Button } from "./ui/button";

export function SettingsPanel(): JSX.Element | null {
  const open = useUiStore((s) => s.settingsOpen);
  const setOpen = useUiStore((s) => s.setSettingsOpen);

  if (!open) return null;

  return (
    <div
      className="fixed inset-0 z-40 flex justify-end bg-black/40"
      onClick={() => setOpen(false)}
      role="presentation"
    >
      <aside
        className="w-[24rem] max-w-[90vw] h-full overflow-y-auto border-l border-slate-800 bg-slate-950 p-4"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between mb-4">
          <h2 className="text-sm font-semibold uppercase tracking-wider text-slate-300">
            Settings
          </h2>
          <Button variant="ghost" onClick={() => setOpen(false)}>
            Close
          </Button>
        </header>
        <AddRepositoryForm />
      </aside>
    </div>
  );
}

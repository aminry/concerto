// Minimal Dialog primitive. We deliberately skip `@radix-ui/react-dialog`
// in V0.1 (Task 25 drift note): the rest of the UI is hand-rolled
// shadcn-look-alikes and the dialog surface is small enough to spell
// out. Phase 3 polish (Task 46+) promotes to the real shadcn dialog.

import type { ReactNode } from "react";
import { X } from "lucide-react";

export type DialogProps = {
  open: boolean;
  onClose: () => void;
  title?: string;
  children: ReactNode;
};

export function Dialog({ open, onClose, title, children }: DialogProps) {
  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      onClick={onClose}
      role="presentation"
    >
      <div
        className="w-[28rem] max-w-[90vw] rounded-lg border border-border bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
      >
        <header className="flex items-center justify-between px-4 py-3 border-b border-border">
          <h2 className="text-sm font-semibold text-foreground">
            {title ?? "Dialog"}
          </h2>
          <button
            type="button"
            onClick={onClose}
            className="text-faint hover:text-foreground transition-colors"
            aria-label="Close"
          >
            <X size={16} />
          </button>
        </header>
        <div className="px-4 py-3 text-sm text-foreground">{children}</div>
      </div>
    </div>
  );
}

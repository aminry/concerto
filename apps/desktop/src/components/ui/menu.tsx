// Anchored dropdown menu. No Radix — hand-rolled to match the other ui/
// primitives. Renders a trigger button; clicking toggles a popover of
// items below it. Closes on outside-click, Escape, or item select.

import { useEffect, useRef, useState, type ReactNode } from "react";

export type MenuItem = {
  id: string;
  label: string;
  description?: string;
  icon?: ReactNode;
};

export function Menu({
  trigger,
  items,
  onSelect,
  align = "left",
}: {
  trigger: (open: boolean) => ReactNode;
  items: ReadonlyArray<MenuItem>;
  onSelect: (id: string) => void;
  align?: "left" | "right";
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function onDoc(e: MouseEvent): void {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    }
    function onKey(e: KeyboardEvent): void {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div ref={ref} className="relative inline-flex">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="inline-flex"
      >
        {trigger(open)}
      </button>
      {open && (
        <div
          role="menu"
          className={`absolute top-full z-50 mt-1 min-w-[12rem] rounded-lg border border-border bg-surface p-1 shadow-xl ${
            align === "right" ? "right-0" : "left-0"
          }`}
        >
          {items.map((it) => (
            <button
              key={it.id}
              type="button"
              role="menuitem"
              onClick={() => {
                onSelect(it.id);
                setOpen(false);
              }}
              className="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-xs text-foreground transition-colors hover:bg-accent hover:text-accent-fg"
            >
              {it.icon}
              <span className="font-medium">{it.label}</span>
              {it.description && (
                <span className="ml-auto text-faint">{it.description}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

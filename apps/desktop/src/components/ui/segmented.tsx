// Segmented control (Split/Unified). Pill background with a raised active
// segment.

export function Segmented<T extends string>({
  items,
  active,
  onSelect,
}: {
  items: ReadonlyArray<{ id: T; label: string }>;
  active: T;
  onSelect: (id: T) => void;
}) {
  return (
    <div className="inline-flex gap-0.5 rounded-md bg-surface-2 p-0.5">
      {items.map((it) => {
        const isActive = it.id === active;
        const cls = isActive
          ? "bg-surface text-foreground shadow-sm"
          : "text-muted hover:text-foreground";
        return (
          <button
            key={it.id}
            type="button"
            aria-pressed={isActive}
            onClick={() => onSelect(it.id)}
            className={`rounded px-2.5 py-1 text-xs font-medium transition-colors ${cls}`}
          >
            {it.label}
          </button>
        );
      })}
    </div>
  );
}

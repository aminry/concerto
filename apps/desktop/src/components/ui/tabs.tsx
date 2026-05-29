// Underline tab strip (Chat/Terminal, Diff/Checks/PR). Generic over the
// tab id string. Active tab gets the accent underline.

export type TabItem<T extends string> = {
  id: T;
  label: string;
  disabled?: boolean;
  title?: string;
};

export function Tabs<T extends string>({
  items,
  active,
  onSelect,
}: {
  items: ReadonlyArray<TabItem<T>>;
  active: T;
  onSelect: (id: T) => void;
}) {
  return (
    <div className="flex items-center gap-1 border-b border-border">
      {items.map((t) => {
        const isActive = t.id === active;
        const cls = isActive
          ? "border-accent text-foreground"
          : t.disabled
            ? "border-transparent text-faint cursor-not-allowed"
            : "border-transparent text-muted hover:text-foreground";
        return (
          <button
            key={t.id}
            type="button"
            disabled={t.disabled}
            title={t.title}
            aria-pressed={isActive}
            onClick={() => !t.disabled && onSelect(t.id)}
            className={`-mb-px border-b-2 px-3 py-1.5 text-xs font-medium transition-colors ${cls}`}
          >
            {t.label}
          </button>
        );
      })}
    </div>
  );
}

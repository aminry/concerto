// Minimal Progress primitive. `value` is 0..100; out-of-range inputs
// clamp. See `button.tsx` for the rationale on skipping the shadcn CLI
// in V0.1.

export type ProgressProps = {
  value: number;
  className?: string;
};

export function Progress({ value, className }: ProgressProps) {
  const clamped = Math.max(0, Math.min(100, value));
  const outer = [
    "h-2 w-full overflow-hidden rounded bg-slate-800",
    className ?? "",
  ]
    .join(" ")
    .trim();
  return (
    <div className={outer}>
      <div
        className="h-full bg-emerald-500 transition-[width]"
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}

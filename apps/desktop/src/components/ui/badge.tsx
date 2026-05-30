// Badge / chip. `neutral` for branch slugs & metadata, `accent` for the
// brand-tinted variant. Defaults to mono since most uses are slugs/IDs.

import type { HTMLAttributes } from "react";

export type BadgeVariant = "neutral" | "accent";

const VARIANTS: Record<BadgeVariant, string> = {
  neutral: "bg-surface-2 text-muted border-border",
  accent: "bg-accent/10 text-accent border-accent/30",
};

export function Badge({
  variant = "neutral",
  className = "",
  ...props
}: HTMLAttributes<HTMLSpanElement> & { variant?: BadgeVariant }) {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-mono ${VARIANTS[variant]} ${className}`}
      {...props}
    />
  );
}

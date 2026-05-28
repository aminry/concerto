// Minimal card primitive. See `button.tsx` for the rationale on
// skipping the shadcn CLI in V0.1. The class strings here are the
// minimum that gets the right slate background + subtle border.

import type { HTMLAttributes } from "react";

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  const combined = ["rounded-md border border-slate-800 bg-slate-900", className ?? ""]
    .join(" ")
    .trim();
  return <div className={combined} {...props} />;
}

export function CardHeader({
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  const combined = ["px-4 py-3 border-b border-slate-800", className ?? ""]
    .join(" ")
    .trim();
  return <div className={combined} {...props} />;
}

export function CardTitle({
  className,
  ...props
}: HTMLAttributes<HTMLHeadingElement>) {
  const combined = [
    "text-sm font-semibold uppercase tracking-wider text-slate-300",
    className ?? "",
  ]
    .join(" ")
    .trim();
  return <h2 className={combined} {...props} />;
}

export function CardContent({
  className,
  ...props
}: HTMLAttributes<HTMLDivElement>) {
  const combined = ["px-4 py-3 text-sm text-slate-200", className ?? ""]
    .join(" ")
    .trim();
  return <div className={combined} {...props} />;
}

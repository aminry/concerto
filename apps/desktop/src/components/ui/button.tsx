// Minimal button primitive. We deliberately skip the full shadcn/ui
// install in V0.1 (Task 24 drift note): `pnpm dlx shadcn@latest init`
// requires interactive input and would scaffold dialog/input/sidebar
// components the V0.1 UI does not exercise. The two components we
// actually need (button, card) are inlined here with hardcoded
// Tailwind class strings. Phase 3 polish promotes this to the full
// shadcn set.

import { forwardRef, type ButtonHTMLAttributes } from "react";

export type ButtonVariant = "default" | "ghost" | "outline";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
};

const VARIANTS: Record<ButtonVariant, string> = {
  default:
    "bg-slate-700 hover:bg-slate-600 text-slate-100 disabled:opacity-50",
  ghost: "bg-transparent hover:bg-slate-800 text-slate-200",
  outline:
    "border border-slate-700 hover:bg-slate-800 text-slate-200 disabled:opacity-50",
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = "default", className, ...props }, ref) => {
    const base =
      "inline-flex items-center justify-center rounded px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed focus:outline-none focus:ring-2 focus:ring-slate-500";
    const variantClass = VARIANTS[variant];
    const combined = [base, variantClass, className ?? ""].join(" ").trim();
    return <button ref={ref} className={combined} {...props} />;
  },
);
Button.displayName = "Button";

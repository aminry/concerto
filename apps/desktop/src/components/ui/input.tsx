// Minimal text input primitive. See `button.tsx` for the rationale on
// skipping the shadcn CLI in V0.1.

import { forwardRef, type InputHTMLAttributes } from "react";

export type InputProps = InputHTMLAttributes<HTMLInputElement>;

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ className, ...props }, ref) => {
    const base =
      "w-full rounded border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none focus:ring-2 focus:ring-slate-500 disabled:opacity-50";
    const combined = [base, className ?? ""].join(" ").trim();
    return <input ref={ref} className={combined} {...props} />;
  },
);
Input.displayName = "Input";

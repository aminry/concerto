// Minimal text input primitive. See `button.tsx` for the rationale on
// skipping the shadcn CLI in V0.1.

import { forwardRef, type InputHTMLAttributes } from "react";

export type InputProps = InputHTMLAttributes<HTMLInputElement>;

export const Input = forwardRef<HTMLInputElement, InputProps>(
  ({ className, ...props }, ref) => {
    const base =
      "w-full rounded-md border border-border-strong bg-background px-2.5 py-1.5 text-sm text-foreground placeholder:text-faint focus:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50";
    const combined = [base, className ?? ""].join(" ").trim();
    return <input ref={ref} className={combined} {...props} />;
  },
);
Input.displayName = "Input";

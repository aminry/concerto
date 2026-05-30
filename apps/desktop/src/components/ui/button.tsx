// Button primitive. Token-driven; variants cover the app's needs.
// `primary` is the indigo accent CTA; `icon` size is for icon-only buttons.

import { forwardRef, type ButtonHTMLAttributes } from "react";

export type ButtonVariant =
  | "default" | "primary" | "ghost" | "outline" | "danger";
export type ButtonSize = "sm" | "md" | "icon";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
};

const VARIANTS: Record<ButtonVariant, string> = {
  default: "bg-surface-2 hover:bg-raised text-foreground disabled:opacity-50",
  primary: "bg-accent hover:bg-accent-hover text-accent-fg disabled:opacity-50",
  ghost: "bg-transparent hover:bg-surface-2 text-muted hover:text-foreground",
  outline:
    "border border-border-strong hover:bg-surface-2 text-foreground disabled:opacity-50",
  danger: "bg-err/10 hover:bg-err/20 text-err disabled:opacity-50",
};

const SIZES: Record<ButtonSize, string> = {
  sm: "px-2 py-1 text-xs",
  md: "px-3 py-1.5 text-sm",
  icon: "h-8 w-8 p-0",
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = "default", size = "md", className, ...props }, ref) => {
    const base =
      "inline-flex items-center justify-center gap-1.5 rounded-md font-medium transition-colors disabled:cursor-not-allowed focus:outline-none focus-visible:ring-2 focus-visible:ring-accent";
    const combined = [base, VARIANTS[variant], SIZES[size], className ?? ""]
      .join(" ")
      .trim();
    return <button ref={ref} className={combined} {...props} />;
  },
);
Button.displayName = "Button";

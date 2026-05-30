// Icon button = square Button(size=icon, variant=ghost) wrapped in a
// Tooltip. `label` is both the tooltip text and the accessible name.

import type { ReactNode } from "react";
import { Button, type ButtonProps } from "./button";
import { Tooltip } from "./tooltip";

export type IconButtonProps = Omit<ButtonProps, "size" | "children"> & {
  label: string;
  side?: "top" | "right" | "bottom" | "left";
  children: ReactNode; // the icon
};

export function IconButton({
  label,
  side = "bottom",
  variant = "ghost",
  children,
  ...props
}: IconButtonProps) {
  return (
    <Tooltip label={label} side={side}>
      <Button size="icon" variant={variant} aria-label={label} {...props}>
        {children}
      </Button>
    </Tooltip>
  );
}

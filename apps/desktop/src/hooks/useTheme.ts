// Theme hook + one-shot controller.
//
// `useTheme()` is a read+actions view over the shared `useThemeStore`, so
// EVERY consumer (StatusBar toggle, xterm, Monaco) observes the same
// effective theme — a local-state hook would desync them.
//
// `useThemeController()` owns the side-effects (OS-preference listener +
// writing `data-theme` onto <html>) and MUST be mounted exactly once,
// near the app root.

import { useEffect } from "react";
import {
  resolveTheme,
  type EffectiveTheme,
  type ThemePreference,
} from "../theme/resolveTheme";
import { useThemeStore } from "../state/useThemeStore";

export type UseThemeResult = {
  preference: ThemePreference;
  effective: EffectiveTheme;
  setPreference: (p: ThemePreference) => void;
  /** system → light → dark → system */
  cycle: () => void;
};

export function useTheme(): UseThemeResult {
  const preference = useThemeStore((s) => s.preference);
  const systemDark = useThemeStore((s) => s.systemDark);
  const setPreference = useThemeStore((s) => s.setPreference);

  const effective = resolveTheme(preference, systemDark);

  const cycle = (): void => {
    setPreference(
      preference === "system"
        ? "light"
        : preference === "light"
          ? "dark"
          : "system",
    );
  };

  return { preference, effective, setPreference, cycle };
}

/// Side-effects for the theme system. Mount once at the app root.
/// Subscribes to the OS color-scheme and writes `data-theme` to <html>
/// whenever the effective theme changes.
export function useThemeController(): void {
  const preference = useThemeStore((s) => s.preference);
  const systemDark = useThemeStore((s) => s.systemDark);
  const setSystemDark = useThemeStore((s) => s.setSystemDark);

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent): void => setSystemDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [setSystemDark]);

  const effective = resolveTheme(preference, systemDark);
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", effective);
  }, [effective]);
}

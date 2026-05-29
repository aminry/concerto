// Theme controller hook. Owns the user preference (persisted), watches the
// OS color-scheme, and writes `data-theme` onto <html>. Returns the
// preference, the effective theme, a setter, and a cycle helper for the
// toggle.

import { useCallback, useEffect, useState } from "react";
import {
  isThemePreference,
  resolveTheme,
  THEME_STORAGE_KEY,
  type EffectiveTheme,
  type ThemePreference,
} from "../theme/resolveTheme";

function loadPreference(): ThemePreference {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(raw) ? raw : "system";
  } catch {
    return "system";
  }
}

function systemPrefersDark(): boolean {
  return (
    typeof window !== "undefined" &&
    !!window.matchMedia &&
    window.matchMedia("(prefers-color-scheme: dark)").matches
  );
}

export type UseThemeResult = {
  preference: ThemePreference;
  effective: EffectiveTheme;
  setPreference: (p: ThemePreference) => void;
  /** system → light → dark → system */
  cycle: () => void;
};

export function useTheme(): UseThemeResult {
  const [preference, setPreferenceState] =
    useState<ThemePreference>(loadPreference);
  const [systemDark, setSystemDark] = useState<boolean>(systemPrefersDark);

  // Track OS changes so `system` preference stays live.
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  const effective = resolveTheme(preference, systemDark);

  // Apply to <html> whenever the effective theme changes.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", effective);
  }, [effective]);

  const setPreference = useCallback((p: ThemePreference) => {
    setPreferenceState(p);
    try {
      localStorage.setItem(THEME_STORAGE_KEY, p);
    } catch {
      // Persistence is best-effort; in-memory state still applies.
    }
  }, []);

  const cycle = useCallback(() => {
    setPreference(
      preference === "system" ? "light" : preference === "light" ? "dark" : "system",
    );
  }, [preference, setPreference]);

  return { preference, effective, setPreference, cycle };
}

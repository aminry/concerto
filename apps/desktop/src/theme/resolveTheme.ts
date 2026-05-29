// Pure theme-resolution logic. No React, no DOM mutation — kept pure so
// it is trivially correct and matches the index.html pre-paint guard's
// mental model. `ThemePreference` is what the user picks; `EffectiveTheme`
// is what actually renders.

export type ThemePreference = "system" | "light" | "dark";
export type EffectiveTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "concerto.theme.v1";

/** Resolve the user's preference + the OS signal into the theme to render. */
export function resolveTheme(
  pref: ThemePreference,
  systemPrefersDark: boolean,
): EffectiveTheme {
  if (pref === "dark") return "dark";
  if (pref === "light") return "light";
  return systemPrefersDark ? "dark" : "light";
}

/** Narrow an untrusted localStorage string back to a ThemePreference. */
export function isThemePreference(v: unknown): v is ThemePreference {
  return v === "system" || v === "light" || v === "dark";
}

// Shared theme state. Lives in its own Zustand store (not useUiStore) so
// the theme preference persists under its own key and every consumer —
// the StatusBar toggle, the xterm terminal, the Monaco diff editor —
// observes the SAME preference/system signal. Component-local state here
// would desync the toggle from the terminal/editor (they'd keep the old
// theme until remount).

import { create } from "zustand";
import {
  isThemePreference,
  THEME_STORAGE_KEY,
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

export type ThemeStore = {
  preference: ThemePreference;
  systemDark: boolean;
  setPreference: (p: ThemePreference) => void;
  setSystemDark: (dark: boolean) => void;
};

export const useThemeStore = create<ThemeStore>((set) => ({
  preference: loadPreference(),
  systemDark: systemPrefersDark(),
  setPreference: (p) => {
    set({ preference: p });
    try {
      localStorage.setItem(THEME_STORAGE_KEY, p);
    } catch {
      // best-effort persistence; in-memory still applies
    }
  },
  setSystemDark: (dark) => set({ systemDark: dark }),
}));

// useAutoUpdate — Task 53.
//
// Daily auto-update check per design/15 §3.9. On mount the hook calls
// `check()` once; while the window stays open it re-checks every 24h.
// On a hit the hook surfaces an `available` flag + an `installAndRestart`
// callback the UI wires up to a non-blocking toast (see
// `components/UpdateToast.tsx`).
//
// The runtime behavior is fully gated on the `plugins.updater` config
// in `tauri.conf.json`: with `endpoints: []` (the V0.1 default), the
// `check()` call resolves to `null` and the hook silently no-ops. The
// signed-bundle release flow (`dist/RELEASE.md`) flips the endpoints +
// pubkey at distribution time.

import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";

/// 24h in ms — the daily-check cadence the design fixes.
const DAILY_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

export interface AutoUpdateState {
  /// The pending update, or `null` if no update is available (or the
  /// updater is unconfigured — `endpoints: []`).
  update: Update | null;
  /// True while an `installAndRestart()` call is in flight; the UI uses
  /// this to disable the toast button so the user can't double-click.
  installing: boolean;
  /// Triggers the download + install + relaunch flow. Safe to call when
  /// `update` is null (it becomes a no-op).
  installAndRestart: () => Promise<void>;
}

export function useAutoUpdate(): AutoUpdateState {
  const [update, setUpdate] = useState<Update | null>(null);
  const [installing, setInstalling] = useState(false);

  useEffect(() => {
    let cancelled = false;

    async function probe(): Promise<void> {
      try {
        const result = await check();
        if (!cancelled) {
          setUpdate(result ?? null);
        }
      } catch {
        // `check()` throws when the updater plugin is misconfigured
        // (e.g. empty pubkey + non-empty endpoints during dev). The
        // hook intentionally swallows — auto-update is best-effort and
        // must never block the shell.
      }
    }

    void probe();
    const handle = window.setInterval(() => {
      void probe();
    }, DAILY_CHECK_INTERVAL_MS);

    return () => {
      cancelled = true;
      window.clearInterval(handle);
    };
  }, []);

  async function installAndRestart(): Promise<void> {
    if (!update || installing) return;
    setInstalling(true);
    try {
      // `downloadAndInstall` streams progress events; we don't wire a
      // progress bar in V0.1 — the toast just shows "Installing…" until
      // the relaunch happens. On macOS the plugin relaunches the app
      // itself once the install completes.
      await update.downloadAndInstall();
    } catch {
      // Install failed; reset so the user can dismiss the toast.
      if (!installing) return;
    } finally {
      setInstalling(false);
    }
  }

  return { update, installing, installAndRestart };
}

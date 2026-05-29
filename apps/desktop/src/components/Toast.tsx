// Self-dismissing toast. V0.1 uses this for the first-run
// `which claude` probe; if no binary is on PATH the toast appears for
// 5 seconds and then auto-fades. The Task 53 update toast piggybacks
// on the same visual style but is action-bearing (Restart button).
// Anything more elaborate (queueing, stacked toasts) is deferred to
// the V1.0 polish pass.

import { useEffect, useState } from "react";

import { checkCommand } from "../api/client";
import { useAutoUpdate } from "../hooks/useAutoUpdate";

export function FirstRunClaudeToast(): JSX.Element | null {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const path = await checkCommand("claude");
        if (!cancelled && !path) {
          setVisible(true);
          // Auto-dismiss after 5s.
          window.setTimeout(() => {
            if (!cancelled) setVisible(false);
          }, 5000);
        }
      } catch {
        // The check is a nice-to-have; silently ignore probe errors.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  if (!visible) return null;

  return (
    <div className="fixed bottom-4 right-4 z-50 max-w-xs rounded-md border border-amber-700 bg-amber-950/90 px-3 py-2 text-xs text-amber-200 shadow-lg">
      Install Claude Code to use sessions.
      <button
        type="button"
        className="ml-2 text-amber-300 hover:text-amber-100"
        onClick={() => setVisible(false)}
        aria-label="Dismiss"
      >
        ×
      </button>
    </div>
  );
}

/// Auto-update toast (Task 53). Non-blocking, action-bearing. Sits at
/// `bottom-16` so it stacks above the first-run claude toast if both
/// happen to fire on the same launch (rare, but cheap to support).
export function AutoUpdateToast(): JSX.Element | null {
  const { update, installing, installAndRestart } = useAutoUpdate();
  const [dismissed, setDismissed] = useState(false);

  if (!update || dismissed) return null;

  return (
    <div className="fixed bottom-16 right-4 z-50 max-w-xs rounded-md border border-emerald-700 bg-emerald-950/90 px-3 py-2 text-xs text-emerald-200 shadow-lg">
      <div className="mb-1 font-semibold">
        Update available: {update.version}
      </div>
      <div className="flex items-center gap-2">
        <button
          type="button"
          className="rounded bg-emerald-700 px-2 py-0.5 text-emerald-50 hover:bg-emerald-600 disabled:opacity-50"
          onClick={() => {
            void installAndRestart();
          }}
          disabled={installing}
        >
          {installing ? "Installing…" : "Restart to update"}
        </button>
        <button
          type="button"
          className="text-emerald-300 hover:text-emerald-100"
          onClick={() => setDismissed(true)}
          aria-label="Dismiss"
        >
          Later
        </button>
      </div>
    </div>
  );
}

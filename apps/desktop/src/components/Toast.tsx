// Self-dismissing toast. V0.1 only uses this for the first-run
// `which claude` probe; if no binary is on PATH the toast appears for
// 5 seconds and then auto-fades. Anything more elaborate (queueing,
// stacked toasts, action buttons) is deferred to the V1.0 polish pass.

import { useEffect, useState } from "react";

import { checkCommand } from "../api/client";

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

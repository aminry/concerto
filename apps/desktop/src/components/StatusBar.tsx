// Bottom status bar (design/15 §3.4). Shows Core connection, current
// branch + active session count for the selected workarea, the permission
// mode, and the theme toggle. Data that isn't surfaced by the Core yet
// shows a static placeholder — wiring those is out of scope for the
// redesign (noted inline).

import { Moon, Sun, MonitorSmartphone, GitBranch } from "lucide-react";
import { useTheme } from "../hooks/useTheme";
import { StatusDot } from "./ui/status-dot";

export function StatusBar(): JSX.Element {
  const { preference, cycle } = useTheme();

  const ThemeIcon =
    preference === "dark" ? Moon : preference === "light" ? Sun : MonitorSmartphone;
  const themeLabel =
    preference === "dark" ? "Dark" : preference === "light" ? "Light" : "System";

  return (
    <footer className="flex h-6 shrink-0 items-center gap-4 border-t border-border bg-surface px-3 text-xs text-muted">
      {/* Connection state: placeholder until the renderer surfaces the
          transport status. Kept on the connected default. */}
      <span className="flex items-center gap-1.5">
        <StatusDot status="ok" />
        Core connected
      </span>
      <span className="flex items-center gap-1.5 font-mono">
        <GitBranch size={12} />
        {/* TODO-data: replace with selected workarea branch when surfaced */}
        —
      </span>
      <div className="ml-auto flex items-center gap-4">
        <span>
          Permission: <span className="text-foreground">plan</span>
        </span>
        <button
          type="button"
          onClick={cycle}
          className="flex items-center gap-1.5 text-muted transition-colors hover:text-foreground"
          title={`Theme: ${themeLabel} (click to change)`}
        >
          <ThemeIcon size={13} />
          {themeLabel}
        </button>
      </div>
    </footer>
  );
}

// Status dot. Maps a semantic status to a token color and exposes an
// accessible label. Used by session tabs, the sidebar tree, the workarea
// header, CI checks, and the status bar.

export type DotStatus = "ok" | "running" | "warning" | "error" | "idle";

const COLOR: Record<DotStatus, string> = {
  ok: "bg-ok",
  running: "bg-run",
  warning: "bg-warn",
  error: "bg-err",
  idle: "bg-faint",
};

const LABEL: Record<DotStatus, string> = {
  ok: "Active", running: "Running", warning: "Warning",
  error: "Error", idle: "Idle",
};

export function StatusDot({
  status,
  className = "",
}: {
  status: DotStatus;
  className?: string;
}) {
  return (
    <span
      className={`inline-block h-2 w-2 shrink-0 rounded-full ${COLOR[status]} ${className}`}
      role="img"
      aria-label={LABEL[status]}
      title={LABEL[status]}
    />
  );
}

// Mobile design tokens (Task 508). A small, self-contained palette for the RN
// component tree. Per PHASE5_PLANNING D11 / design/16 amendment, mobile builds
// its OWN component tree and does NOT consume the web/desktop `@concerto/ui`
// renderer — so these tokens are deliberately mobile-local, not shared.
export const colors = {
  bg: "#0e1116",
  surface: "#161b22",
  surfaceAlt: "#1c2230",
  border: "#2a3340",
  text: "#e6edf3",
  textMuted: "#8b949e",
  accent: "#6e7bf2",
  // Severity tints (mirror the notifications `severity` field: low|medium|high).
  sevLow: "#3fb950",
  sevMedium: "#d29922",
  sevHigh: "#f85149",
  // Status/state tints reused by the Workspaces drill-down (Task 513).
  success: "#3fb950",
  warning: "#d29922",
  danger: "#f85149",
  info: "#58a6ff",
  merged: "#a371f7",
} as const;

export const spacing = {
  xs: 4,
  sm: 8,
  md: 12,
  lg: 16,
  xl: 24,
} as const;

export const radius = {
  sm: 6,
  md: 10,
  lg: 14,
} as const;

/** Map a notification `severity` string to its tint. */
export function severityColor(severity: string): string {
  switch (severity) {
    case "high":
      return colors.sevHigh;
    case "medium":
      return colors.sevMedium;
    default:
      return colors.sevLow;
  }
}

/**
 * Map a workarea/session `status` string to a status-pill tint (Task 513).
 * Workarea statuses ∈ created|active|running|awaiting|paused|finished|partial|
 * archived|crashed; session statuses ∈ starting|running|awaiting|finished|crashed.
 */
export function statusColor(status: string): string {
  switch (status) {
    case "running":
    case "active":
    case "starting":
      return colors.success;
    case "awaiting":
    case "paused":
    case "partial":
      return colors.warning;
    case "crashed":
      return colors.danger;
    case "finished":
      return colors.info;
    default:
      return colors.textMuted;
  }
}

/**
 * Map a PR `state` string to a tint (Task 513). GitHub provider states ∈
 * open|closed|merged|draft.
 */
export function prStateColor(state: string): string {
  switch (state) {
    case "open":
      return colors.success;
    case "merged":
      return colors.merged;
    case "closed":
      return colors.danger;
    default:
      return colors.textMuted;
  }
}

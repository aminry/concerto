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

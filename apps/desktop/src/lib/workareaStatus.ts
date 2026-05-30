// Single source of truth mapping a `concerto.v1.Workarea` status to a
// StatusDot color, so the sidebar tree and the center-panel header always
// agree. Workarea statuses ∈ { created | active | running | awaiting |
// paused | archived | crashed }.
//   active   → green (live / healthy)
//   running  → blue  (an agent is actively executing)
//   awaiting → amber (needs input)
//   crashed  → red
//   created | paused | archived → grey
import type { DotStatus } from "../components/ui/status-dot";

export function workareaStatusToDot(status: string): DotStatus {
  switch (status) {
    case "active":
      return "ok";
    case "running":
      return "running";
    case "awaiting":
      return "warning";
    case "crashed":
      return "error";
    case "created":
    case "paused":
    case "archived":
      return "idle";
    default:
      return "idle";
  }
}

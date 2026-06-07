// Single source of truth mapping a `concerto.v1.Workarea` status to a
// StatusDot color, so the sidebar tree, the center-panel header, and the
// workspace summary (Task 323) always agree. Workarea statuses ∈
// { created | active | running | awaiting | paused | finished | partial |
// archived | crashed } — Task 307 widened the set with `finished` and
// `partial`.
//   active   → green (live / healthy)
//   running  → blue  (an agent is actively executing)
//   awaiting → amber (needs input)
//   partial  → amber (a parallel attempt completed only some of its work —
//              a warning state the user should review, Task 307)
//   crashed  → red
//   finished → grey (the attempt is done; idle, not live — Task 307)
//   created | paused | archived → grey
import type { DotStatus } from "../components/ui/status-dot";

export function workareaStatusToDot(status: string): DotStatus {
  switch (status) {
    case "active":
      return "ok";
    case "running":
      return "running";
    case "awaiting":
    case "partial":
      return "warning";
    case "crashed":
      return "error";
    case "created":
    case "paused":
    case "finished":
    case "archived":
      return "idle";
    default:
      return "idle";
  }
}

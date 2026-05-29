// One pill in the session tab strip.
//
// Renders the agent kind + a status-tinted dot. The dot is driven by
// `useSessionEvents(sid)` which subscribes to `session.events.<sid>`
// and collapses the V0.1 oneof set into one of four badge values.

import type { Session } from "../api/sessions";
import {
  useSessionEvents,
  type SessionStatusBadge,
} from "../hooks/useSessionEvents";
import { StatusDot, type DotStatus } from "./ui/status-dot";

export type SessionTabProps = {
  session: Session;
  active: boolean;
  onClick: () => void;
};

export function SessionTab({
  session,
  active,
  onClick,
}: SessionTabProps): JSX.Element {
  // Map the persisted Session.status into a badge baseline; the live
  // event stream overrides it when the session is actively running
  // in this Desktop instance.
  const initial = mapPersistedStatus(session.status);
  const { status } = useSessionEvents(session.id, initial);

  const buttonClass = active
    ? "px-3 py-1 text-xs rounded-md border border-accent bg-accent/10 text-foreground flex items-center gap-2"
    : "px-3 py-1 text-xs rounded-md border border-border bg-surface text-muted hover:bg-surface-2 flex items-center gap-2";

  return (
    <button type="button" className={buttonClass} onClick={onClick}>
      <StatusDot status={sessionStatusToDot(status)} />
      <span className="truncate max-w-[10rem]">{session.agent_kind}</span>
      <span className="font-mono text-faint truncate max-w-[6rem]">
        {session.id.slice(0, 8)}
      </span>
    </button>
  );
}

// Map the session status — both the live `SessionStatusBadge` values
// that drive this tab's dot and the raw persisted `Session.status`
// vocabulary ({ starting | running | awaiting | finished | crashed })
// — onto the semantic `StatusDot` palette.
function sessionStatusToDot(status: SessionStatusBadge | string): DotStatus {
  switch (status) {
    case "running":
    case "starting":
      return "running";
    case "awaiting":
      return "warning";
    case "finished":
    case "exited":
    case "stopped":
    case "done":
      return "idle";
    case "crashed":
    case "failed":
    case "error":
      return "error";
    default:
      return "idle";
  }
}

function mapPersistedStatus(status: string): SessionStatusBadge {
  switch (status) {
    case "running":
    case "awaiting":
      return "running";
    case "finished":
      return "finished";
    case "crashed":
      return "crashed";
    case "starting":
    default:
      return "starting";
  }
}

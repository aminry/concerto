// One pill in the session tab strip.
//
// Renders the agent kind + a status-tinted dot. The dot is driven by
// `useSessionEvents(sid)` which subscribes to `session.events.<sid>`
// and collapses the V0.1 oneof set into one of four badge values.

import { X } from "lucide-react";

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
  onClose: () => void;
};

export function SessionTab({
  session,
  active,
  onClick,
  onClose,
}: SessionTabProps): JSX.Element {
  // Map the persisted Session.status into a badge baseline; the live
  // event stream overrides it when the session is actively running
  // in this Desktop instance.
  const initial = mapPersistedStatus(session.status);
  const { status } = useSessionEvents(session.id, initial);

  const cellClass = active
    ? "group relative shrink-0 h-9 px-3 flex items-center gap-2 border-r border-border text-xs cursor-pointer bg-background text-foreground"
    : "group relative shrink-0 h-9 px-3 flex items-center gap-2 border-r border-border text-xs cursor-pointer text-muted hover:bg-surface-2";

  return (
    <div className={cellClass} onClick={onClick}>
      {active && (
        <span className="absolute inset-x-0 top-0 h-0.5 bg-accent" />
      )}
      <StatusDot status={sessionStatusToDot(status)} />
      <span className="truncate max-w-[10rem]">{session.agent_kind}</span>
      <span
        className="font-mono text-faint truncate max-w-[6rem]"
        title={session.id}
      >
        {/* UUIDv7 ids share a long leading timestamp prefix for sessions
            created close together, so slicing from the front shows the
            same chars on every tab. Show the trailing (random) segment,
            which is distinct per session; full id is in the tooltip. */}
        {session.id.slice(-6)}
      </span>
      <button
        type="button"
        aria-label="Close session"
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        className={`grid h-4 w-4 place-items-center rounded text-faint hover:bg-err/20 hover:text-err ${active ? "" : "opacity-0 group-hover:opacity-100"}`}
      >
        <X size={12} />
      </button>
    </div>
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

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
    ? "px-3 py-1 text-xs rounded bg-slate-700 text-slate-100 flex items-center gap-2"
    : "px-3 py-1 text-xs rounded bg-slate-900 text-slate-300 hover:bg-slate-800 flex items-center gap-2";

  return (
    <button type="button" className={buttonClass} onClick={onClick}>
      <StatusDot status={status} />
      <span className="truncate max-w-[10rem]">{session.agent_kind}</span>
      <span className="text-slate-500 truncate max-w-[6rem]">
        {session.id.slice(0, 8)}
      </span>
    </button>
  );
}

function StatusDot({ status }: { status: SessionStatusBadge }): JSX.Element {
  const color = badgeColor(status);
  return (
    <span
      className={`inline-block h-2 w-2 rounded-full ${color}`}
      aria-label={`session status: ${status}`}
    />
  );
}

function badgeColor(status: SessionStatusBadge): string {
  switch (status) {
    case "running":
      return "bg-blue-500";
    case "finished":
      return "bg-slate-500";
    case "crashed":
      return "bg-rose-500";
    case "starting":
    default:
      return "bg-amber-500";
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

// The Concerto-chat transcript (Task 415). Renders the `maestro.message` chat
// lines and the QUOTED session-response surfacing (design/08 §3.5: routed
// session output is shown back as quoted lines, e.g. "Routed to bach / Claude →
// …").
//
// The transcript is a render of `MaestroEvent`s the parent lifts off the
// `maestro.events` subscription (the live emitter is Task 414; until then the
// stream is empty behind the mocked `invoke`, which is the deliberate UX seam —
// the empty-state below, NOT a stub). The events themselves are
// server-canonical; this component is a pure renderer of the lifted list.

import type { MaestroEvent, MaestroTurn } from "../../api/maestro";

/// A single rendered transcript line. The parent maps the `MaestroEvent` union
/// into this view shape; `routing_executed` becomes a `quoted` line.
export type TranscriptLine = {
  id: string;
  kind: "message" | "quoted" | "notice";
  text: string;
  /// For `quoted` lines: the routed-target label ("bach / Claude").
  source?: string;
  role?: string;
};

/// Map the decoded `MaestroEvent` stream into renderable transcript lines.
/// `digest_generated` / `budget_exhausted` / `disabled_by_policy` are surfaced
/// elsewhere (the digest panel + banners), so the transcript keeps only the
/// conversational events. Pure — unit-tested.
export function eventsToLines(events: MaestroEvent[]): TranscriptLine[] {
  const lines: TranscriptLine[] = [];
  events.forEach((ev, i) => {
    if (ev.kind === "message") {
      lines.push({
        id: `msg-${i}`,
        kind: "message",
        text: ev.text,
        role: ev.role,
      });
    } else if (ev.kind === "routing_executed") {
      // design/08 §3.5: surfaced back as a quoted line.
      const source = ev.targets.join(", ");
      lines.push({
        id: `route-${i}`,
        kind: "quoted",
        text: ev.summary ?? `Routed to ${source}`,
        source,
      });
    }
  });
  return lines;
}

/// Map the persisted Maestro history (Task 8) into renderable transcript lines.
/// Each `MaestroTurn` becomes a `message` line tagged with its persisted role,
/// so a reload rebuilds the conversation top-to-bottom (oldest-first) exactly
/// as the live `message` events render. Pure — unit-tested. Ids are
/// `hist-`-prefixed so they never collide with the live `msg-`/`route-` ids.
export function historyToLines(turns: MaestroTurn[]): TranscriptLine[] {
  return turns.map((turn, i) => ({
    id: `hist-${i}`,
    kind: "message",
    text: turn.text,
    role: turn.role,
  }));
}

/// Whether the "Maestro is working" indicator should be ON after observing
/// `event`. A `role:"user"` message means the turn was just forwarded to the
/// model, so a reply is being prepared (ON); a `role:"assistant"` message means
/// the reply arrived (OFF); routing dispatch / budget / policy stops mean no
/// reply is coming (OFF). Returns `null` for events that don't change the
/// waiting state (digest refreshes, unknown frames). Pure — unit-tested.
export function waitingAfterEvent(event: MaestroEvent): boolean | null {
  switch (event.kind) {
    case "message":
      return event.role === "user";
    case "routing_executed":
    case "budget_exhausted":
    case "disabled_by_policy":
      return false;
    default:
      return null;
  }
}

/// Pure "is the scroll container pinned to the bottom?" decision for the
/// auto-scroll behavior. The transcript should follow new messages ONLY when the
/// user is already at (or within `threshold` px of) the bottom — so appending a
/// reply never yanks someone who has scrolled up to read earlier history. When
/// the content is shorter than the viewport there is nothing to scroll, which
/// counts as "at the bottom". Unit-tested; the DOM glue lives in `MaestroChat`.
export function isNearBottom(
  m: { scrollTop: number; scrollHeight: number; clientHeight: number },
  threshold = 24,
): boolean {
  return m.scrollHeight - m.scrollTop - m.clientHeight <= threshold;
}

export type MaestroTranscriptProps = {
  lines: TranscriptLine[];
  /// When true, append an animated "Maestro is working" bubble after the last
  /// line — the conversation is waiting on a streamed reply.
  busy?: boolean;
};

/// The animated "Maestro is preparing a response" bubble (three bouncing dots),
/// styled like an assistant message so it reads as the model starting to type.
function WorkingIndicator(): JSX.Element {
  return (
    <li className="flex flex-col items-start" data-testid="maestro-typing">
      <span className="mb-0.5 px-1 text-[10px] font-semibold uppercase tracking-wide text-accent">
        Maestro
      </span>
      <div
        role="status"
        aria-label="Maestro is preparing a response"
        className="flex items-center gap-1 rounded-2xl rounded-bl-sm border border-border bg-raised px-3 py-2.5 shadow-sm"
      >
        {[0, 150, 300].map((delay) => (
          <span
            key={delay}
            className="h-1.5 w-1.5 animate-bounce rounded-full bg-faint"
            style={{ animationDelay: `${delay}ms` }}
          />
        ))}
      </div>
    </li>
  );
}

/// Is this turn the local user's (right-aligned, accent-tinted) vs the
/// Maestro/assistant's (left-aligned, neutral)? Anything that isn't an explicit
/// "user" role renders as the agent side.
function isUserRole(role?: string): boolean {
  return role?.toLowerCase() === "user";
}

export function MaestroTranscript({
  lines,
  busy = false,
}: MaestroTranscriptProps): JSX.Element {
  // Empty state only when there is nothing to show AND nothing is in flight —
  // while busy we render the working indicator instead (the user's turn is
  // already on its way to the model).
  if (lines.length === 0 && !busy) {
    return (
      <div
        className="px-4 py-6 text-center text-sm text-faint"
        data-testid="transcript-empty"
      >
        No messages yet. Ask the Concerto chat about your workareas, or route a
        prompt with <span className="font-mono text-muted">@workarea</span>.
      </div>
    );
  }
  return (
    <ul
      className="flex flex-col gap-3 px-3 py-3"
      data-testid="transcript"
    >
      {lines.map((line) => {
        // Routed-session output: a distinct, full-width quoted confirmation.
        if (line.kind === "quoted") {
          return (
            <li key={line.id} className="flex justify-center">
              <div
                className="max-w-[92%] rounded-lg border border-accent/30 bg-accent/5 px-3 py-2 text-sm text-muted"
                data-role="quoted"
              >
                {line.source && (
                  <span className="mr-1.5 font-mono text-xs font-medium text-accent">
                    ↳ {line.source}
                  </span>
                )}
                <span className="whitespace-pre-wrap">{line.text}</span>
              </div>
            </li>
          );
        }

        const user = isUserRole(line.role);
        return (
          <li
            key={line.id}
            className={`flex flex-col ${user ? "items-end" : "items-start"}`}
            data-role={user ? "user" : "assistant"}
          >
            <span
              className={`mb-0.5 px-1 text-[10px] font-semibold uppercase tracking-wide ${
                user ? "text-faint" : "text-accent"
              }`}
            >
              {user ? "You" : "Maestro"}
            </span>
            <div
              className={`max-w-[85%] whitespace-pre-wrap rounded-2xl px-3 py-2 text-sm leading-relaxed shadow-sm ${
                user
                  ? "rounded-br-sm bg-accent/10 text-foreground"
                  : "rounded-bl-sm border border-border bg-raised text-foreground"
              }`}
            >
              {line.text}
            </div>
          </li>
        );
      })}
      {busy && <WorkingIndicator />}
    </ul>
  );
}

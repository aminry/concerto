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

export type MaestroTranscriptProps = {
  lines: TranscriptLine[];
};

export function MaestroTranscript({
  lines,
}: MaestroTranscriptProps): JSX.Element {
  if (lines.length === 0) {
    return (
      <div
        className="px-3 py-4 text-sm text-faint"
        data-testid="transcript-empty"
      >
        No messages yet. Ask the Concerto chat about your workareas, or route a
        prompt with <span className="font-mono text-muted">@workarea</span>.
      </div>
    );
  }
  return (
    <ul className="flex flex-col gap-2 px-3 py-2" data-testid="transcript">
      {lines.map((line) => (
        <li key={line.id}>
          {line.kind === "quoted" ? (
            <blockquote className="border-l-2 border-accent/50 pl-2 text-sm text-muted">
              {line.source && (
                <span className="mr-1 font-mono text-xs text-accent">
                  Routed to {line.source} →
                </span>
              )}
              <span className="whitespace-pre-wrap">{line.text}</span>
            </blockquote>
          ) : (
            <div className="text-sm text-foreground">
              {line.role && (
                <span className="mr-1 text-xs font-semibold uppercase text-faint">
                  {line.role}
                </span>
              )}
              <span className="whitespace-pre-wrap">{line.text}</span>
            </div>
          )}
        </li>
      ))}
    </ul>
  );
}

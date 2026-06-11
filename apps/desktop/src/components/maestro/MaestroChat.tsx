// The always-present "Concerto chat" top bar (Task 415, design/08 §1).
//
// Mounted ABOVE the three-panel `PanelGroup` (in `App.tsx`) so it persists
// across workspace/workarea selection — it is the outer agent the whole app
// talks to. It holds, top→bottom: the budget/policy banners, the digest panel,
// the transcript, the pending write-tool confirmation chip, and the composer.
// The whole region is collapsible (the bar is always MOUNTED; collapse only
// hides its body, design/08 §1).
//
// ── State ownership (design/15 §3.3) ─────────────────────────────────────────
// SERVER-CANONICAL (React Query / live stream):
//   - the digest → `getDigest`, invalidated on a `digest_generated` event;
//   - the transcript → accumulated from the `maestro.events` stream;
//   - the banner triggers → `budget_exhausted` / `disabled_by_policy` events.
// UI-ONLY (`useMaestroStore` Zustand): composer draft, collapse flags, the
// pending-confirmation SELECTION.
//
// ── The mocked-invoke seam (Tier-2) ──────────────────────────────────────────
// The live `Maestro.*` shell dispatch arm + the `maestro.events` emitter are
// Task 414's. Until 414 lands, `getDigest` rejects (`Status::unimplemented`)
// behind the real shell and is mocked in tests; the stream is empty. The
// resulting empty-state renders (no digest / no messages) are the DELIBERATE
// UX seam, NOT stubs — they light up with zero rework when 414 wires live data.

import { useCallback, useMemo, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import {
  decodeMaestroEvent,
  getDigest,
  MAESTRO_EVENTS_SUBJECT,
  type Digest,
  type MaestroEvent,
} from "../../api/maestro";
import { useEventSubscription } from "../../hooks/useEventSubscription";
import { useMaestroStore } from "../../state/useMaestroStore";
import { BudgetBanner } from "./BudgetBanner";
import { ConfirmationChip } from "./ConfirmationChip";
import { DigestPanel } from "./DigestPanel";
import { MaestroComposer } from "./MaestroComposer";
import {
  eventsToLines,
  MaestroTranscript,
  type TranscriptLine,
} from "./MaestroTranscript";

export const MAESTRO_DIGEST_QUERY_KEY = ["maestro", "digest"] as const;

export function MaestroChat(): JSX.Element {
  const queryClient = useQueryClient();

  const chatCollapsed = useMaestroStore((s) => s.chatCollapsed);
  const toggleChatCollapsed = useMaestroStore((s) => s.toggleChatCollapsed);
  const digestCollapsed = useMaestroStore((s) => s.digestCollapsed);
  const toggleDigestCollapsed = useMaestroStore((s) => s.toggleDigestCollapsed);
  const pendingConfirmation = useMaestroStore((s) => s.pendingConfirmation);
  const setPendingConfirmation = useMaestroStore(
    (s) => s.setPendingConfirmation,
  );

  // Conversational events accumulated off the live stream (server-canonical).
  const [events, setEvents] = useState<MaestroEvent[]>([]);
  const [exhaustedByEvent, setExhaustedByEvent] = useState(false);
  const [policyDisabledReason, setPolicyDisabledReason] = useState<
    string | null
  >(null);

  // The digest is React-Query-canonical. `getDigest` rejects with
  // `Status::unimplemented` until 414; React Query holds the error and the
  // panel degrades to its empty state (the deliberate seam). `retry: false`
  // is the App-level default, so a rejected query doesn't thrash the shell.
  const digestQuery = useQuery<Digest>({
    queryKey: MAESTRO_DIGEST_QUERY_KEY,
    queryFn: getDigest,
  });

  // Subscribe to `maestro.events`; decode each opaque frame defensively and
  // fold it into the renderer state. `digest_generated` invalidates the digest
  // query (the `useEventSubscription` invalidation pattern); the banner events
  // flip the local triggers. The live emitter is 414 — empty until then.
  const onFrame = useCallback(
    (payload: unknown) => {
      const ev = decodeMaestroEvent(payload);
      switch (ev.kind) {
        case "message":
        case "routing_executed":
          setEvents((prev) => [...prev, ev]);
          break;
        case "digest_generated":
          void queryClient.invalidateQueries({
            queryKey: MAESTRO_DIGEST_QUERY_KEY,
          });
          break;
        case "budget_exhausted":
          setExhaustedByEvent(true);
          break;
        case "disabled_by_policy":
          setPolicyDisabledReason(
            ev.reason ??
              "Concerto chat disabled by enterprise data-privacy policy.",
          );
          break;
        case "unknown":
          break;
      }
    },
    [queryClient],
  );
  useEventSubscription<unknown>(MAESTRO_EVENTS_SUBJECT, onFrame);

  const lines: TranscriptLine[] = useMemo(
    () => eventsToLines(events),
    [events],
  );

  const refreshDigest = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: MAESTRO_DIGEST_QUERY_KEY });
  }, [queryClient]);

  return (
    <div
      className="flex flex-col border-b border-border bg-surface"
      data-testid="maestro-chat"
      aria-label="Concerto chat"
    >
      <header className="flex items-center gap-2 px-3 py-1.5">
        <button
          type="button"
          onClick={toggleChatCollapsed}
          className="flex items-center gap-1.5 text-sm font-semibold text-foreground hover:text-accent"
          aria-expanded={!chatCollapsed}
        >
          {chatCollapsed ? (
            <ChevronRight size={14} />
          ) : (
            <ChevronDown size={14} />
          )}
          Concerto chat
        </button>
      </header>

      {!chatCollapsed && (
        <div className="flex flex-col">
          <BudgetBanner
            state={null}
            budget={null}
            exhaustedByEvent={exhaustedByEvent}
            policyDisabledReason={policyDisabledReason}
          />
          <DigestPanel
            digest={digestQuery.data ?? null}
            collapsed={digestCollapsed}
            onToggleCollapsed={toggleDigestCollapsed}
            onRefresh={refreshDigest}
            refreshing={digestQuery.isFetching}
          />
          <div className="max-h-48 overflow-auto">
            <MaestroTranscript lines={lines} />
          </div>
          {pendingConfirmation && (
            <div className="px-3 pb-1">
              <ConfirmationChip
                sessionId={pendingConfirmation.sessionId}
                approval={pendingConfirmation.approval}
                onResolved={() => setPendingConfirmation(null)}
              />
            </div>
          )}
          <MaestroComposer />
        </div>
      )}
    </div>
  );
}

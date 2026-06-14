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

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import {
  decodeMaestroEvent,
  getDigest,
  getHistory,
  getState,
  MAESTRO_EVENTS_SUBJECT,
  type Digest,
  type MaestroEvent,
  type MaestroState,
  type MaestroTurn,
} from "../../api/maestro";
import { useEventSubscription } from "../../hooks/useEventSubscription";
import { useMaestroStore } from "../../state/useMaestroStore";
import { BudgetBanner } from "./BudgetBanner";
import { ConfirmationChip } from "./ConfirmationChip";
import { DigestPanel } from "./DigestPanel";
import { MaestroComposer } from "./MaestroComposer";
import {
  eventsToLines,
  historyToLines,
  isNearBottom,
  MaestroTranscript,
  waitingAfterEvent,
  type TranscriptLine,
} from "./MaestroTranscript";
import { useMaestroConfirmations } from "./useMaestroConfirmations";

export const MAESTRO_DIGEST_QUERY_KEY = ["maestro", "digest"] as const;
export const MAESTRO_STATE_QUERY_KEY = ["maestro", "state"] as const;

/// Derive the single `budget` number `<BudgetBanner>` compares against. The
/// banner computes `max(daily_in_today, daily_out_today) / budget`, so the
/// faithful cap to pair with the larger counter is that dimension's own cap
/// (`in_cap` for input-token-bound, `out_cap` for output-token-bound). This
/// lights amber/red on whichever dimension is closest to its cap without
/// fabricating any value. Returns null when there is no state (banner falls
/// back to the event-driven path).
export function deriveBudget(state: MaestroState | null | undefined): number | null {
  if (!state) return null;
  return state.daily_in_today >= state.daily_out_today
    ? state.in_cap
    : state.out_cap;
}

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
  // The persisted chat history (Task 8), loaded ONCE on mount so the
  // conversation survives a reload. Live `message`/`routing_executed` events
  // append AFTER these history lines. Loading once (rather than merging on
  // every event) keeps the seed deterministic; a transient duplicate of a
  // just-sent turn that lands in BOTH the history fetch and a live event is
  // acceptable (and rare — the fetch resolves on mount, before new turns).
  const [history, setHistory] = useState<MaestroTurn[]>([]);
  // True between forwarding the user's turn to the model and the streamed reply
  // landing — drives the "Maestro is working" indicator. Derived from the event
  // stream (a `role:"user"` message turns it on; the assistant reply / routing /
  // budget|policy stop turns it off) so it tracks the real round-trip, not just
  // the (instant) `SendToMaestro` ack.
  const [waitingForReply, setWaitingForReply] = useState(false);
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

  // The live Maestro state (Task 416's `Maestro.GetState`) is
  // React-Query-canonical. It feeds the budget meter (counts vs caps), the
  // inert/stale badge + policy-disabled banner, and — critically — carries the
  // Maestro singleton session id the confirmation-chip producer subscribes to.
  // Invalidated on the banner-driving events below; when the handle is
  // policy-disabled the Core rejects with `disabled_by_policy` and the query
  // holds the error (the policy banner still lights via the event path).
  const stateQuery = useQuery<MaestroState>({
    queryKey: MAESTRO_STATE_QUERY_KEY,
    queryFn: getState,
  });
  const maestroState = stateQuery.data ?? null;

  // The live state can itself report a policy disable (`inert_reason ==
  // "disabled_by_policy"`) consistently with the `disabled_by_policy` event;
  // surface either as the policy banner. The event-driven reason wins when set
  // (it carries the human reason string).
  const statePolicyReason =
    maestroState?.inert_reason === "disabled_by_policy"
      ? "Concerto chat disabled by enterprise data-privacy policy."
      : null;

  // The confirmation-chip PRODUCER (design/08 R-2): subscribe to the Maestro
  // session's `session.events.<sid>` and lift write-tool `AwaitingApproval`
  // frames into `pendingConfirmation`. Empty session id ⇒ no subscription.
  useMaestroConfirmations(maestroState?.maestro_session_id);

  // Subscribe to `maestro.events`; decode each opaque frame defensively and
  // fold it into the renderer state. `digest_generated` invalidates the digest
  // query (the `useEventSubscription` invalidation pattern); the banner events
  // flip the local triggers. The live emitter is 414 — empty until then.
  const onFrame = useCallback(
    (payload: unknown) => {
      const ev = decodeMaestroEvent(payload);
      // Flip the working indicator on the user→assistant round-trip.
      const waiting = waitingAfterEvent(ev);
      if (waiting !== null) setWaitingForReply(waiting);
      switch (ev.kind) {
        case "message":
        case "routing_executed":
          setEvents((prev) => [...prev, ev]);
          break;
        case "digest_generated":
          void queryClient.invalidateQueries({
            queryKey: MAESTRO_DIGEST_QUERY_KEY,
          });
          // A fresh digest also advances `last_digest_at_ms` in the state.
          void queryClient.invalidateQueries({
            queryKey: MAESTRO_STATE_QUERY_KEY,
          });
          break;
        case "budget_exhausted":
          setExhaustedByEvent(true);
          // Re-read the live state so `inert`/counters reflect exhaustion.
          void queryClient.invalidateQueries({
            queryKey: MAESTRO_STATE_QUERY_KEY,
          });
          break;
        case "disabled_by_policy":
          setPolicyDisabledReason(
            ev.reason ??
              "Concerto chat disabled by enterprise data-privacy policy.",
          );
          void queryClient.invalidateQueries({
            queryKey: MAESTRO_STATE_QUERY_KEY,
          });
          break;
        case "unknown":
          break;
      }
    },
    [queryClient],
  );
  useEventSubscription<unknown>(MAESTRO_EVENTS_SUBJECT, onFrame);

  // Seed the transcript with the persisted history ONCE on mount (Task 8). The
  // Core skips the text-less checkpoint marker rows, so every turn here renders.
  // `getHistory` rejects with `Status::unimplemented`/policy-disabled behind the
  // mocked shell until the Maestro is live — we swallow the error and start
  // empty (the deliberate seam; live events still populate the transcript).
  useEffect(() => {
    let cancelled = false;
    void getHistory()
      .then((turns) => {
        if (!cancelled) setHistory(turns);
      })
      .catch(() => {
        /* no persisted history (policy-disabled / not yet live) — start empty */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // History (persisted, oldest-first) seeds the top; live events append below.
  const lines: TranscriptLine[] = useMemo(
    () => [...historyToLines(history), ...eventsToLines(events)],
    [history, events],
  );

  const refreshDigest = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: MAESTRO_DIGEST_QUERY_KEY });
  }, [queryClient]);

  // Auto-scroll the transcript to the newest message — but ONLY when the user
  // is already pinned to the bottom, so appending a reply never yanks someone
  // who has scrolled up to read earlier history. `pinnedRef` starts true so the
  // initial history seed lands at the latest turn. `useLayoutEffect` scrolls
  // before paint to avoid a visible jump.
  const transcriptRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const onTranscriptScroll = useCallback(() => {
    const el = transcriptRef.current;
    if (el) pinnedRef.current = isNearBottom(el);
  }, []);
  useLayoutEffect(() => {
    const el = transcriptRef.current;
    if (el && pinnedRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [lines, waitingForReply]);

  // Safety net: if a reply never lands (model hang / dropped stream), clear the
  // working indicator after a generous window so it never spins forever. Each
  // new turn (waiting → true) resets the timer.
  useEffect(() => {
    if (!waitingForReply) return;
    const t = setTimeout(() => setWaitingForReply(false), 180_000);
    return () => clearTimeout(t);
  }, [waitingForReply]);

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
            state={maestroState}
            budget={deriveBudget(maestroState)}
            exhaustedByEvent={exhaustedByEvent}
            policyDisabledReason={policyDisabledReason ?? statePolicyReason}
          />
          <DigestPanel
            digest={digestQuery.data ?? null}
            inert={maestroState?.inert ?? false}
            collapsed={digestCollapsed}
            onToggleCollapsed={toggleDigestCollapsed}
            onRefresh={refreshDigest}
            refreshing={digestQuery.isFetching}
          />
          <div
            ref={transcriptRef}
            onScroll={onTranscriptScroll}
            className="max-h-64 min-h-[88px] overflow-auto"
          >
            <MaestroTranscript lines={lines} busy={waitingForReply} />
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

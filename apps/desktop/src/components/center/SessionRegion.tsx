// Top region of the center panel — session tab strip + xterm panel +
// composer.
//
// Carved out of the Task 26 `WorkareaDetail` so the Task 46 three-panel
// layout can sit it inside a resizable region. The behaviour is
// unchanged: auto-select the first session, the active session's
// terminal mounts, the composer below sends bytes via
// `Sessions.SendMessage`.

import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../../state/useUiStore";
import { useSessions } from "../../hooks/useSessions";
import { useEventSubscription } from "../../hooks/useEventSubscription";
import {
  createSession,
  deleteSession,
  oneofVariant,
  type Session,
  type SessionEventPayload,
  type StreamEvent,
} from "../../api/sessions";
import { errorMessage } from "../../api/client";
import { formatError } from "../../api/errors";
import { SessionTab } from "../SessionTab";
import { SessionTerminal } from "../SessionTerminal";
import { SessionComposer } from "../SessionComposer";
import { Menu } from "../ui/menu";
import { Dialog } from "../ui/dialog";
import { Button } from "../ui/button";
import { Plus, TerminalSquare } from "lucide-react";

export type SessionRegionProps = {
  workareaId: string;
};

export function SessionRegion({ workareaId }: SessionRegionProps): JSX.Element {
  const activeSessionId = useUiStore((s) => s.activeSessionId);
  const setActiveSession = useUiStore((s) => s.setActiveSession);
  const queryClient = useQueryClient();

  const sessionsQuery = useSessions(workareaId);
  // Core returns sessions newest-first (`ORDER BY started_at DESC`); the
  // tab strip reads left-to-right, so sort ascending here to put the
  // oldest tab on the left and a freshly created session on the right.
  const sessions = [...(sessionsQuery.data?.sessions ?? [])].sort(
    (a, b) => startedAtMillis(a) - startedAtMillis(b),
  );

  const [confirmSession, setConfirmSession] = useState<Session | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // Task 323: the per-workarea edit-mutex contention notice for the active
  // session ("blocked on <other session>"). Surfaced when the Core reports
  // a write was serialized away — see the subscription below. Non-blocking
  // + dismissible; the strip stays intact.
  const [contention, setContention] = useState<string | null>(null);

  useEffect(() => {
    if (!activeSessionId && sessions.length > 0) {
      setActiveSession(sessions[0].id);
    }
  }, [activeSessionId, sessions, setActiveSession]);

  const activeSession =
    sessions.find((s) => s.id === activeSessionId) ?? null;

  // Task 323 — surface the per-workarea edit-mutex contention EFFECT (the
  // mutex itself is server-side, Task 308 / design/04 §3.5; this only
  // displays it). Per Task 308's handoff the blocked write rides the
  // existing `session.events` stream as an `ApprovalResolved` whose
  // `decision` string carries the typed `workarea.edit_mutex.blocked`
  // wire-code + the holder description ("blocked on session <id>"). We
  // subscribe to the ACTIVE session's events and lift that description into
  // a dismissible inline notice scoped to that session. No client-side
  // serialization — we only read what the Core already emitted.
  useEffect(() => {
    setContention(null);
  }, [activeSessionId]);
  useEditMutexContention(activeSessionId, setContention);
  const sessionDisabled =
    activeSession === null ||
    !["starting", "running", "awaiting"].includes(activeSession.status);

  const createMutation = useMutation({
    mutationFn: async (agentKind: string) =>
      createSession({ workareaId, agentKind }),
    onSuccess: (session) => {
      setActionError(null);
      setActiveSession(session.id);
      void queryClient.invalidateQueries({
        queryKey: ["sessions", workareaId],
      });
    },
    onError: (e) => setActionError(`Couldn't start session: ${errorMessage(e)}`),
  });

  const deleteMutation = useMutation({
    mutationFn: async (sessionId: string) => deleteSession(sessionId),
    onSuccess: (_, id) => {
      setActionError(null);
      void queryClient.invalidateQueries({
        queryKey: ["sessions", workareaId],
      });
      // Reselect from the pre-invalidation `sessions` snapshot. This
      // assumes serial closes (one ✕ click at a time) — the expected
      // interaction; rapid concurrent deletes could momentarily pick a
      // just-closed id before the refetch lands.
      if (id === activeSessionId) {
        setActiveSession(nextActiveSessionId(sessions, id));
      }
    },
    onError: (e) => setActionError(`Couldn't delete session: ${errorMessage(e)}`),
  });

  function handleClose(s: Session): void {
    const running = ["starting", "running", "awaiting"].includes(s.status);
    if (running) {
      setConfirmSession(s);
      return;
    }
    deleteMutation.mutate(s.id);
  }

  return (
    <section className="h-full flex flex-col min-h-0 p-2 gap-2">
      <div className="shrink-0 flex items-stretch bg-surface border-b border-border">
        <div className="flex items-stretch overflow-x-auto min-w-0">
          {sessions.map((s) => (
            <SessionTab
              key={s.id}
              session={s}
              active={s.id === activeSessionId}
              onClick={() => setActiveSession(s.id)}
              onClose={() => handleClose(s)}
            />
          ))}
          {sessionsQuery.isLoading && (
            <span className="self-center px-3 text-xs text-faint">Loading…</span>
          )}
          {sessionsQuery.isError && (
            <span className="self-center px-3 text-xs text-err">
              {formatError(sessionsQuery.error)}
            </span>
          )}
        </div>
        <NewSessionMenu onPick={(agentKind) => createMutation.mutate(agentKind)} />
      </div>
      {actionError && (
        <div className="shrink-0 flex items-center justify-between gap-2 rounded-md border border-err/40 bg-err/10 px-3 py-1.5 text-xs text-err">
          <span className="truncate">{actionError}</span>
          <button
            type="button"
            onClick={() => setActionError(null)}
            className="text-err/80 hover:text-err shrink-0"
          >
            Dismiss
          </button>
        </div>
      )}
      {contention && (
        <div
          role="status"
          className="shrink-0 flex items-center justify-between gap-2 rounded-md border border-warn/40 bg-warn/10 px-3 py-1.5 text-xs text-warn"
        >
          <span className="truncate">{contention}</span>
          <button
            type="button"
            onClick={() => setContention(null)}
            className="text-warn/80 hover:text-warn shrink-0"
          >
            Dismiss
          </button>
        </div>
      )}
      <div className="flex-1 min-h-0 flex flex-col gap-2">
        {activeSession ? (
          <>
            <SessionTerminal
              key={activeSession.id}
              sessionId={activeSession.id}
              disabled={sessionDisabled}
            />
            <SessionComposer
              sessionId={activeSession.id}
              disabled={sessionDisabled}
            />
          </>
        ) : (
          <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-3 text-faint text-sm border border-dashed border-border rounded-lg">
            <TerminalSquare size={28} />
            No sessions yet.
            <NewSessionMenu
              onPick={(k) => createMutation.mutate(k)}
              primary
            />
          </div>
        )}
      </div>
      <Dialog
        open={confirmSession !== null}
        onClose={() => setConfirmSession(null)}
        title="Delete running session?"
      >
        <p className="text-sm text-muted">
          This session is still running. Stop the agent and permanently delete
          the session — including its transcript, approvals, and checkpoints?
          This can’t be undone.
        </p>
        <div className="mt-4 flex justify-end gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setConfirmSession(null)}
          >
            Cancel
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={() => {
              const id = confirmSession?.id;
              setConfirmSession(null);
              if (id) deleteMutation.mutate(id);
            }}
          >
            Stop &amp; delete
          </Button>
        </div>
      </Dialog>
    </section>
  );
}

/// Sort key for a session's start time. `started_at` is a
/// `[seconds, nanos]` tuple; a session still being created may not have
/// one yet — sort those last so the newest tab lands on the right.
function startedAtMillis(s: Session): number {
  if (!s.started_at) return Number.MAX_SAFE_INTEGER;
  const [secs, nanos] = s.started_at;
  return secs * 1000 + nanos / 1e6;
}

/// Pick the next active session after `closedId` is removed: prefer the
/// tab immediately before it, else the first remaining session, else
/// null (no sessions left).
function nextActiveSessionId(
  sessions: Session[],
  closedId: string,
): string | null {
  const idx = sessions.findIndex((s) => s.id === closedId);
  const remaining = sessions.filter((s) => s.id !== closedId);
  if (remaining.length === 0) return null;
  if (idx > 0) {
    const prev = sessions[idx - 1];
    if (prev.id !== closedId) return prev.id;
  }
  return remaining[0].id;
}

/// The end-of-strip "new session" affordance. Default form is a `+`
/// cell that matches the tab height; `primary` swaps it for a primary
/// CTA used in the empty state.
function NewSessionMenu({
  onPick,
  primary = false,
}: {
  onPick: (agentKind: string) => void;
  primary?: boolean;
}): JSX.Element {
  return (
    <Menu
      align="right"
      label="New session"
      trigger={() =>
        primary ? (
          <span className="inline-flex items-center gap-1.5 rounded-md bg-accent hover:bg-accent-hover text-accent-fg px-2 py-1 text-xs font-medium">
            <Plus size={14} />
            New session
          </span>
        ) : (
          <span
            className="grid h-9 w-9 place-items-center text-muted hover:text-accent hover:bg-surface-2"
            title="New session"
          >
            <Plus size={16} />
          </span>
        )
      }
      items={AGENT_MENU_ITEMS}
      onSelect={onPick}
    />
  );
}

/// The user-creatable agent set for the "+ new session" menu (Task 323,
/// design/15 §3.4). FROZEN: these `id`s are the `agent_kind` strings passed
/// to `createSession` and MUST match the Core's `sessions.agent_kind` CHECK
/// spelling exactly. The CHECK set is `('claude','codex','gemini','maestro')`;
/// `maestro` is the P4-internal orchestrator (Task 415), not a
/// user-creatable tab, so it is excluded here. The V0.1 `echo` smoke agent
/// is dropped from the menu (smoke.sh creates its echo session directly via
/// the Core, not through this menu — see Handoff).
const AGENT_MENU_ITEMS = [
  { id: "claude", label: "claude", description: "Claude Code" },
  { id: "codex", label: "codex", description: "OpenAI Codex" },
  { id: "gemini", label: "gemini", description: "Gemini CLI" },
] as const;

/// Subscribe to the active session's `session.events.<sid>` stream and lift
/// any per-workarea edit-mutex contention ("blocked on <other session>")
/// into the `onContention` setter. The mutex is server-side (Task 308); the
/// blocked write rides the existing stream as an `ApprovalResolved` whose
/// `decision` string starts with the `workarea.edit_mutex.blocked`
/// wire-code (see SessionRegion's note). We parse that string and surface a
/// human-readable notice. Read-only — no client-side serialization.
function useEditMutexContention(
  sessionId: string | null,
  onContention: (msg: string) => void,
): void {
  useEventSubscription<StreamEvent>(
    sessionId ? `session.events.${sessionId}` : "",
    (event) => {
      // Oneof variants serialize PascalCase by prost's serde default;
      // `oneofVariant` accepts both spellings.
      const session = oneofVariant<SessionEventPayload>(
        event.body,
        "Session",
        "session",
      );
      const kind = session?.kind;
      if (!kind) return;
      // `ApprovalResolved` isn't in the V0.1 SessionEventPayload.kind union;
      // read it dynamically. Its `decision` string carries the typed
      // wire-code + holder description when a write was serialized away.
      const resolved = oneofVariant<{ decision?: string }>(
        kind,
        "ApprovalResolved",
        "approval_resolved",
      );
      const decision = resolved?.decision ?? "";
      if (decision.startsWith(EDIT_MUTEX_BLOCKED_WIRE_CODE)) {
        // `decision` is "workarea.edit_mutex.blocked: blocked on session
        // <id>"; show the human half after the wire-code prefix.
        const detail =
          decision.slice(EDIT_MUTEX_BLOCKED_WIRE_CODE.length + 2).trim() ||
          "blocked on another session";
        onContention(`Edit serialized — ${detail}`);
      }
    },
  );
}

/// Typed wire-code the Core (Task 308) prefixes onto the `ApprovalResolved`
/// decision string when a write-class tool call is rejected because another
/// session on the same workarea holds the edit mutex. Mirrors
/// `EDIT_MUTEX_BLOCKED_WIRE_CODE` in the Core's `workspace_manager`.
const EDIT_MUTEX_BLOCKED_WIRE_CODE = "workarea.edit_mutex.blocked";

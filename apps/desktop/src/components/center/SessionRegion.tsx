// Top region of the center panel — session tab strip + xterm panel +
// composer.
//
// Carved out of the Task 26 `WorkareaDetail` so the Task 46 three-panel
// layout can sit it inside a resizable region. The behaviour is
// unchanged: auto-select the first session, the active session's
// terminal mounts, the composer below sends bytes via
// `Sessions.SendMessage`.

import { useEffect } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../../state/useUiStore";
import { useSessions } from "../../hooks/useSessions";
import { createSession, deleteSession, type Session } from "../../api/sessions";
import { SessionTab } from "../SessionTab";
import { SessionTerminal } from "../SessionTerminal";
import { SessionComposer } from "../SessionComposer";
import { Menu } from "../ui/menu";
import { Plus, TerminalSquare } from "lucide-react";

export type SessionRegionProps = {
  workareaId: string;
};

export function SessionRegion({ workareaId }: SessionRegionProps): JSX.Element {
  const activeSessionId = useUiStore((s) => s.activeSessionId);
  const setActiveSession = useUiStore((s) => s.setActiveSession);
  const queryClient = useQueryClient();

  const sessionsQuery = useSessions(workareaId);
  const sessions = sessionsQuery.data?.sessions ?? [];

  useEffect(() => {
    if (!activeSessionId && sessions.length > 0) {
      setActiveSession(sessions[0].id);
    }
  }, [activeSessionId, sessions, setActiveSession]);

  const activeSession =
    sessions.find((s) => s.id === activeSessionId) ?? null;
  const sessionDisabled =
    activeSession === null ||
    !["starting", "running", "awaiting"].includes(activeSession.status);

  const createMutation = useMutation({
    mutationFn: async (agentKind: string) =>
      createSession({ workareaId, agentKind }),
    onSuccess: (session) => {
      setActiveSession(session.id);
      void queryClient.invalidateQueries({
        queryKey: ["sessions", workareaId],
      });
    },
  });

  const deleteMutation = useMutation({
    mutationFn: async (sessionId: string) => deleteSession(sessionId),
    onSuccess: (_, id) => {
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
  });

  function handleClose(s: Session): void {
    const running = ["starting", "running", "awaiting"].includes(s.status);
    if (
      running &&
      !window.confirm(
        "Stop and delete this running session? This permanently removes its transcript.",
      )
    ) {
      return;
    }
    deleteMutation.mutate(s.id);
  }

  return (
    <section className="h-full flex flex-col min-h-0 p-2 gap-2">
      <div className="shrink-0 flex items-stretch overflow-x-auto bg-surface border-b border-border">
        {sessions.map((s) => (
          <SessionTab
            key={s.id}
            session={s}
            active={s.id === activeSessionId}
            onClick={() => setActiveSession(s.id)}
            onClose={() => handleClose(s)}
          />
        ))}
        <NewSessionMenu onPick={(agentKind) => createMutation.mutate(agentKind)} />
        {sessionsQuery.isLoading && (
          <span className="self-center px-3 text-xs text-faint">Loading…</span>
        )}
        {sessionsQuery.isError && (
          <span className="self-center px-3 text-xs text-err">
            {String(sessionsQuery.error)}
          </span>
        )}
      </div>
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
    </section>
  );
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
      items={[
        { id: "claude", label: "claude", description: "Claude Code" },
        { id: "echo", label: "echo", description: "smoke test" },
      ]}
      onSelect={onPick}
    />
  );
}

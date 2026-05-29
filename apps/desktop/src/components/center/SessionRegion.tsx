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
import { stopSession } from "../../api/sessions";
import { SessionTab } from "../SessionTab";
import { SessionTerminal } from "../SessionTerminal";
import { SessionComposer } from "../SessionComposer";
import { Button } from "../ui/button";
import { Tabs } from "../ui/tabs";
import { TerminalSquare } from "lucide-react";

export type SessionRegionProps = {
  workareaId: string;
};

export function SessionRegion({ workareaId }: SessionRegionProps): JSX.Element {
  const activeSessionId = useUiStore((s) => s.activeSessionId);
  const setActiveSession = useUiStore((s) => s.setActiveSession);
  const setStartSessionPickerOpen = useUiStore(
    (s) => s.setStartSessionPickerOpen,
  );
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

  const stopMutation = useMutation({
    mutationFn: async (sessionId: string) => stopSession(sessionId),
    onSuccess: () =>
      void queryClient.invalidateQueries({
        queryKey: ["sessions", workareaId],
      }),
  });

  return (
    <section className="h-full flex flex-col min-h-0 p-2 gap-2">
      <div className="shrink-0 flex items-center gap-2 flex-wrap">
        <span className="text-xs uppercase tracking-wide text-faint">
          Sessions:
        </span>
        {sessionsQuery.isLoading && (
          <span className="text-xs text-faint">Loading…</span>
        )}
        {sessionsQuery.isError && (
          <span className="text-xs text-err">
            {String(sessionsQuery.error)}
          </span>
        )}
        {sessions.map((s) => (
          <SessionTab
            key={s.id}
            session={s}
            active={s.id === activeSessionId}
            onClick={() => setActiveSession(s.id)}
          />
        ))}
        <Button
          variant="outline"
          size="sm"
          onClick={() => setStartSessionPickerOpen(true)}
        >
          + Start Session
        </Button>
        {activeSession && !sessionDisabled && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => stopMutation.mutate(activeSession.id)}
            disabled={stopMutation.isPending}
          >
            {stopMutation.isPending ? "Stopping…" : "Stop Session"}
          </Button>
        )}
      </div>
      <div className="flex-1 min-h-0 flex flex-col gap-2">
        {activeSession ? (
          <>
            <SubTabHeader />
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
          <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-2 text-faint text-sm border border-dashed border-border rounded-lg">
            <TerminalSquare size={28} />
            No sessions yet. Click “+ Start Session”.
          </div>
        )}
      </div>
    </section>
  );
}

/// Session-level sub-tabs: V0.1 ships Terminal only; Chat is a stub
/// placeholder ("Chat view comes in V1.0") per the task spec. The strip
/// renders even though there is only one live tab so the structure
/// matches the design diagram.
function SubTabHeader(): JSX.Element {
  return (
    <Tabs
      items={[
        { id: "terminal", label: "Terminal" },
        { id: "chat", label: "Chat", disabled: true, title: "Chat view comes in V1.0" },
      ]}
      active="terminal"
      onSelect={() => {}}
    />
  );
}

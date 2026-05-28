// Workarea detail panel — header + session tab strip + xterm panel +
// composer.
//
// Replaces Task 25's JSON placeholder with the V0.1 terminal surface
// from Task 26. The panel is laid out as a vertical flex column so
// the terminal can claim the remaining height (xterm.js needs a
// measurable container; the `min-h-0` rule on each flex child is the
// one that lets the inner panel actually shrink).

import { useEffect } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import { useWorkarea } from "../hooks/useWorkareas";
import { useSessions } from "../hooks/useSessions";
import { stopSession } from "../api/sessions";
import { SessionTab } from "./SessionTab";
import { SessionTerminal } from "./SessionTerminal";
import { SessionComposer } from "./SessionComposer";
import { Button } from "./ui/button";

export function WorkareaDetail(): JSX.Element {
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const activeSessionId = useUiStore((s) => s.activeSessionId);
  const setActiveSession = useUiStore((s) => s.setActiveSession);
  const setStartSessionPickerOpen = useUiStore(
    (s) => s.setStartSessionPickerOpen,
  );
  const queryClient = useQueryClient();

  const workareaQuery = useWorkarea(selectedWorkareaId);
  const sessionsQuery = useSessions(selectedWorkareaId);
  const sessions = sessionsQuery.data?.sessions ?? [];

  // Auto-select the first session when there's no active one — keeps
  // the panel populated after a workarea switch.
  useEffect(() => {
    if (!activeSessionId && sessions.length > 0) {
      setActiveSession(sessions[0].id);
    }
  }, [activeSessionId, sessions, setActiveSession]);

  const activeSession =
    sessions.find((s) => s.id === activeSessionId) ?? null;
  // Per task spec: a stopped session keeps its terminal mounted but
  // the input is greyed. The `agent_kind` carry from list isn't
  // enough — once we hear `AgentExited` over the events stream we
  // disable input even if the persisted row hasn't updated yet.
  // The disabled flag here is the cheaper proxy: anything not
  // `starting` / `running` / `awaiting` is disabled. The terminal
  // panel + composer respect this flag.
  const sessionDisabled =
    activeSession === null ||
    !["starting", "running", "awaiting"].includes(activeSession.status);

  const stopMutation = useMutation({
    mutationFn: async (sessionId: string) => stopSession(sessionId),
    onSuccess: () =>
      void queryClient.invalidateQueries({
        queryKey: ["sessions", selectedWorkareaId],
      }),
  });

  if (!selectedWorkareaId) {
    return (
      <main className="flex-1 p-6 overflow-auto text-slate-400">
        Select a workarea to start a session.
      </main>
    );
  }

  return (
    <main className="flex-1 flex flex-col min-h-0 p-3 gap-2">
      <header className="shrink-0 border-b border-slate-800 pb-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <h2 className="text-sm font-semibold text-slate-200 truncate">
              {workareaQuery.data?.composer_name ?? "Workarea"}
            </h2>
            {workareaQuery.data && (
              <>
                <span className="text-xs px-1.5 py-0.5 rounded bg-slate-800 text-slate-300 font-mono">
                  {workareaQuery.data.branch_name}
                </span>
                <span className="text-xs text-slate-500">
                  {workareaQuery.data.status}
                </span>
              </>
            )}
          </div>
        </div>
        <div className="mt-2 flex items-center gap-2 flex-wrap">
          <span className="text-xs uppercase tracking-wider text-slate-500">
            Sessions:
          </span>
          {sessionsQuery.isLoading && (
            <span className="text-xs text-slate-500">Loading…</span>
          )}
          {sessionsQuery.isError && (
            <span className="text-xs text-rose-400">
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
            onClick={() => setStartSessionPickerOpen(true)}
          >
            + Start Session
          </Button>
          {activeSession && !sessionDisabled && (
            <Button
              variant="ghost"
              onClick={() => stopMutation.mutate(activeSession.id)}
              disabled={stopMutation.isPending}
            >
              {stopMutation.isPending ? "Stopping…" : "Stop Session"}
            </Button>
          )}
        </div>
      </header>

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
        <div className="flex-1 min-h-0 flex items-center justify-center text-slate-500 text-sm border border-dashed border-slate-800 rounded">
          No sessions yet. Click “+ Start Session”.
        </div>
      )}
    </main>
  );
}

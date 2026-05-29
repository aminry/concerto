// "+ Start Session" picker. Two buttons, one per V0.1 agent kind.
// No model picker — V0.1 has no model RPC (Task 26 pre-decision 17).
//
// On confirm, mutates `Sessions.CreateSession`, sets the new session
// as the active tab, and closes the dialog.

import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { createSession } from "../api/sessions";
import { useUiStore } from "../state/useUiStore";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export function StartSessionPicker(): JSX.Element | null {
  const open = useUiStore((s) => s.startSessionPickerOpen);
  const setOpen = useUiStore((s) => s.setStartSessionPickerOpen);
  const workareaId = useUiStore((s) => s.selectedWorkareaId);
  const setActiveSession = useUiStore((s) => s.setActiveSession);
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const mutation = useMutation({
    mutationFn: async (agentKind: string) => {
      if (!workareaId) throw new Error("no workarea selected");
      return createSession({ workareaId, agentKind });
    },
    onSuccess: (session) => {
      setActiveSession(session.id);
      void queryClient.invalidateQueries({
        queryKey: ["sessions", workareaId],
      });
      setOpen(false);
      setError(null);
    },
    onError: (e) => setError(String(e)),
  });

  if (!open) return null;

  return (
    <Dialog
      open={open}
      onClose={() => {
        setOpen(false);
        setError(null);
      }}
      title="Start Session"
    >
      <div className="space-y-3">
        <p className="text-muted text-xs">Pick an agent for this workarea.</p>
        <div className="flex gap-2">
          <Button
            variant="outline"
            onClick={() => mutation.mutate("echo")}
            disabled={mutation.isPending || !workareaId}
          >
            echo (smoke)
          </Button>
          <Button
            variant="primary"
            onClick={() => mutation.mutate("claude")}
            disabled={mutation.isPending || !workareaId}
          >
            claude
          </Button>
        </div>
        {mutation.isPending && (
          <p className="text-xs text-muted">Creating session…</p>
        )}
        {error && <p className="text-xs text-err">{error}</p>}
      </div>
    </Dialog>
  );
}

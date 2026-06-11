// "New Workspace" modal — the primary creation flow after the
// Project→Workspace collapse.
//
// A workspace is a top-level node over the GLOBAL repository registry. This
// modal is a thin create wrapper around the shared `WorkspaceForm`: it owns
// the create mutation and the dialog open/close, while the form owns all the
// field/selection state and the three-source repo picker.
//
// On submit it hands the assembled `WorkspaceFormSubmit` to `createWorkspace`,
// then invalidates the workspace list and selects the new workspace. The form
// is mounted fresh on each open (gated below), so it resets between opens.

import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import { createWorkspace } from "../api/workspaces";
import { formatError } from "../api/errors";
import { Dialog } from "./ui/dialog";
import { WorkspaceForm, type WorkspaceFormSubmit } from "./WorkspaceForm";
import { CreateFromDescription } from "./CreateFromDescription";
import { bootstrapWorkspace } from "./bootstrapWorkspace";

// Back-compat re-export — tests and callers import `deriveRepoName` from here.
export { deriveRepoName } from "./WorkspaceForm";

/// The two create modes the modal offers. `manual` is the long-standing
/// `WorkspaceForm` assembly; `from-description` is the §3.8 Maestro create
/// front door (Task 418) — paste a description / issue link and step through
/// detected repos → suggested cones → confirm.
type CreateMode = "manual" | "from-description";

export function NewWorkspaceModal(): JSX.Element {
  const open = useUiStore((s) => s.newWorkspaceModalOpen);
  const setOpen = useUiStore((s) => s.setNewWorkspaceModalOpen);
  const setSelectedWorkspace = useUiStore((s) => s.setSelectedWorkspace);
  const setWorkspaceExpanded = useUiStore((s) => s.setWorkspaceExpanded);
  const setActiveSession = useUiStore((s) => s.setActiveSession);
  const queryClient = useQueryClient();

  const [mode, setMode] = useState<CreateMode>("manual");

  // Bootstrap runs after the dialog has already closed, so any failure can't
  // be surfaced inline. Kept in state for a future toast; logged for now.
  const [, setBootstrapError] = useState<string | null>(null);

  const mutation = useMutation({
    mutationFn: (values: WorkspaceFormSubmit) =>
      createWorkspace({
        name: values.name,
        icon: values.icon,
        description: values.description,
        repos: values.repos,
      }),
    onSuccess: async (workspace) => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      setSelectedWorkspace(workspace.id);
      setOpen(false);

      // The workspace is already committed; auto-create its first workarea +
      // session so the user lands in a ready-to-use session. The dialog is
      // already closed (above), so a bootstrap failure is surfaced
      // non-blockingly rather than reopening the modal.
      try {
        const { workareaId, sessionId } = await bootstrapWorkspace(workspace.id);
        setWorkspaceExpanded(workspace.id, true);
        setActiveSession(sessionId);
        void queryClient.invalidateQueries({
          queryKey: ["workareas", workspace.id],
        });
        void queryClient.invalidateQueries({
          queryKey: ["sessions", workareaId],
        });
      } catch (e) {
        setBootstrapError(formatError(e));
        console.warn("workspace bootstrap failed:", e);
      }
    },
  });

  // Select + expand the newly-created workspace; used by the from-description
  // mode, which runs its own create + (optional) bootstrap and reports back the
  // workspace id + the bootstrapped session id (null when "just the workspace"
  // was chosen).
  function onWorkspaceCreated(
    workspaceId: string,
    sessionId: string | null,
  ): void {
    setSelectedWorkspace(workspaceId);
    if (sessionId) {
      setWorkspaceExpanded(workspaceId, true);
      setActiveSession(sessionId);
    }
    setMode("manual");
    setOpen(false);
  }

  function close(): void {
    setMode("manual");
    setOpen(false);
  }

  // Mount the form only while open so it resets fresh on every open (this
  // replaces the old reset-on-open effect).
  if (!open) return <></>;

  return (
    <Dialog open={open} onClose={close} title="New Workspace">
      <div className="space-y-4">
        {/* Mode toggle — the discoverable §3.8 "create from description /
            issue link" entry point sits right beside the manual builder. */}
        <div
          role="radiogroup"
          aria-label="Create mode"
          className="flex gap-1 rounded-md border border-border-strong bg-background p-0.5 text-xs"
        >
          <ModeTab
            label="Build manually"
            active={mode === "manual"}
            onClick={() => setMode("manual")}
          />
          <ModeTab
            label="From description / issue link"
            active={mode === "from-description"}
            onClick={() => setMode("from-description")}
          />
        </div>

        {mode === "manual" ? (
          <WorkspaceForm
            mode="create"
            submitLabel="Create Workspace"
            pendingLabel="Creating…"
            pending={mutation.isPending}
            externalError={
              mutation.isError ? formatError(mutation.error) : null
            }
            onCancel={close}
            onSubmit={(v) => mutation.mutate(v)}
          />
        ) : (
          <CreateFromDescription
            onCreated={onWorkspaceCreated}
            onCancel={close}
          />
        )}
      </div>
    </Dialog>
  );
}

function ModeTab({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onClick}
      className={`flex-1 rounded px-2 py-1 transition-colors ${
        active
          ? "bg-accent text-accent-fg"
          : "text-muted hover:text-foreground hover:bg-surface-2"
      }`}
    >
      {label}
    </button>
  );
}

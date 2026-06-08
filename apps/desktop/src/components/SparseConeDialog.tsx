// "Choose directories for the sparse checkout" dialog (design/02 §3.2).
//
// Wraps `RepoTreeBrowser` in a modal with Save/Cancel. On Save it calls
// `Repositories.SetRepoConeDefaults`, which persists the repository's default
// cone AND propagates it to every existing workarea of the repo; on success
// it surfaces an inline "Updated N workarea(s)" note and closes. The dialog
// pre-loads the repo's existing `cone_defaults` as the initial selection so
// re-opening it edits (adds/removes) the current default.

import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  setRepoConeDefaults,
  type Repository,
} from "../api/repositories";
import { formatError } from "../api/errors";
import { normalizeConeSelection, RepoTreeBrowser } from "./RepoTreeBrowser";
import { Dialog } from "./ui/dialog";
import { Button } from "./ui/button";

export type SparseConeDialogProps = {
  open: boolean;
  onClose: () => void;
  repository: Repository;
  /// Invalidated query key to refresh after a successful save (the repo list
  /// so the row's `cone_defaults` re-renders). Optional.
  invalidateKey?: readonly unknown[];
};

export function SparseConeDialog({
  open,
  onClose,
  repository,
  invalidateKey,
}: SparseConeDialogProps): JSX.Element {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<string[]>(
    normalizeConeSelection(repository.cone_defaults ?? []),
  );
  const [savedNote, setSavedNote] = useState<string | null>(null);

  // Re-seed the selection whenever the dialog (re-)opens for a repo so a
  // fresh open always reflects the latest persisted default.
  useEffect(() => {
    if (open) {
      setSelected(normalizeConeSelection(repository.cone_defaults ?? []));
      setSavedNote(null);
    }
  }, [open, repository.id, repository.cone_defaults]);

  const mutation = useMutation({
    mutationFn: () => setRepoConeDefaults(repository.id, selected),
    onSuccess: (res) => {
      const n = res.workareas_updated;
      setSavedNote(`Updated ${n} workarea${n === 1 ? "" : "s"}.`);
      if (invalidateKey) {
        void queryClient.invalidateQueries({ queryKey: invalidateKey });
      }
      // Brief pause so the success note is visible, then close.
      window.setTimeout(onClose, 600);
    },
  });

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Choose directories for the sparse checkout"
    >
      <div className="space-y-3">
        <p className="text-xs text-faint">
          These directories are checked out in every workarea you create from{" "}
          <span className="font-mono text-foreground">{repository.name}</span>.
          Saving updates existing workareas too.
        </p>

        <RepoTreeBrowser
          repositoryId={repository.id}
          value={selected}
          onChange={setSelected}
        />

        {mutation.isError && (
          <p role="alert" className="text-xs text-err">
            {formatError(mutation.error)}
          </p>
        )}
        {savedNote && <p className="text-xs text-ok">{savedNote}</p>}

        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="button"
            variant="primary"
            disabled={mutation.isPending}
            onClick={() => {
              setSavedNote(null);
              mutation.mutate();
            }}
          >
            {mutation.isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

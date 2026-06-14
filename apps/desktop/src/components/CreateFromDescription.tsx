// The §3.8 "create a workspace from a natural-language description / issue
// link" front door (Task 418) — the headline Maestro create flow whose backend
// Task 411 shipped (`Repositories.SuggestCones` + `create_workspace_from_
// description`) but which had NO Desktop UI until now.
//
// ── UX choice (documented in the task Handoff) ───────────────────────────────
// A MODAL STEPPER inside the New Workspace modal, NOT in-chat chips. The task
// allows either ("Pick one coherent UX"); the stepper is chosen because:
//   - it is discoverable: a segmented toggle at the top of `NewWorkspaceModal`
//     flips between "Build manually" (the existing `WorkspaceForm`) and "From
//     description / issue link" (this stepper), so the front door sits exactly
//     where users already create workspaces;
//   - it reuses the existing `ConePicker` verbatim (no second cone widget);
//   - it stays entirely out of `MaestroChat.tsx` / `api/maestro.ts`, which the
//     sibling Task 417 is rebuilding — zero shared-surface collision beyond the
//     one additive `Repositories.SuggestCones` arm in `client.ts`.
//
// ── The never-silent invariant (R-2 / design/08 §3.8 line 221) ───────────────
// Steps 1–3 (describe → detect repos → suggest cones) spend NO create side
// effects. The workspace/workarea is created ONLY when the user clicks one of
// the explicit confirmation-slate buttons in step 4 ("Create workspace + first
// workarea" / "Just the workspace, no workarea"). "Edit repo set / cones" loops
// back to the editable steps. There is no skip-confirm fast path.
//
// ── Tier-2 seam ──────────────────────────────────────────────────────────────
// The detector is deterministic (issue-ref parse + repo-name keyword match over
// the global registry) so it needs zero LLM tokens and is fully CI-provable.
// `suggestCones` is driven against a mocked `invoke`; a Core without an injected
// suggester rejects (UNIMPLEMENTED), which degrades to an empty seed + manual
// entry rather than blocking the flow. Real issue fetch + real cone quality are
// the Phase-4 Tier-3 line, not this task's double.

import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowRight, Link2, Loader2 } from "lucide-react";

import {
  listRepositories,
  suggestCones,
  type Repository,
} from "../api/repositories";
import { createWorkspace } from "../api/workspaces";
import { formatError } from "../api/errors";
import { ConePicker, parseConePaths } from "./ConePicker";
import { bootstrapWorkspace } from "./bootstrapWorkspace";
import { deriveWorkspaceName } from "./workspaceName";
import { Button } from "./ui/button";

/// The create-slate choice the user confirms in step 4 (the design/08 §3.8
/// step-4 options). `with_workarea` and `workspace_only` both COMMIT; `edit`
/// loops back to the editable steps and never creates.
export type CreateSlateAction =
  | "with_workarea"
  | "workspace_only"
  | "edit";

type Step = "describe" | "repos" | "cones" | "confirm";

/// A Linear / GitHub / Jira issue URL pattern (deterministic, zero LLM tokens).
/// Mirrors the parse Task 411's `create_from_description` does server-side; here
/// it only surfaces the detected link in the UI (the real fetch is the Core's /
/// Tier-3 job — the Desktop double does not round-trip the issue).
const ISSUE_URL_RE =
  /\bhttps?:\/\/\S*(?:linear\.app|github\.com|atlassian\.net|jira)\S*/i;

/// Detect the issue URL inside a freeform description, if any. Returns null for
/// a pure freeform description (the §3.8 "no URL → freeform planning" path).
export function detectIssueUrl(text: string): string | null {
  const m = text.match(ISSUE_URL_RE);
  return m ? m[0] : null;
}

/// Deterministically propose the repo subset a description names. A repo is
/// proposed when its name (or a significant token of it) appears as a
/// word-ish substring of the lowercased description ("add SSO to the API and
/// the iOS app" → `api`, `ios`). Ambiguity is never resolved silently: the
/// detected set is a PRE-CHECK only; the user edits it in step 2, and an empty
/// detection still lets the user pick manually. Exported for the Tier-2 test.
export function detectRepos(text: string, repos: Repository[]): string[] {
  const hay = ` ${text.toLowerCase()} `;
  const out: string[] = [];
  for (const r of repos) {
    const name = r.name.toLowerCase().trim();
    if (name.length === 0) continue;
    // Match the whole repo name OR any hyphen/underscore/slash-delimited
    // token of it (so "ios-app" matches "ios"), as a non-alphanumeric-bounded
    // occurrence to avoid "api" matching "rapid".
    const tokens = [name, ...name.split(/[-_/]/)].filter((t) => t.length >= 2);
    const hit = tokens.some((t) =>
      new RegExp(`[^a-z0-9]${escapeRegExp(t)}[^a-z0-9]`).test(hay),
    );
    if (hit) out.push(r.id);
  }
  return out;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export type CreateFromDescriptionProps = {
  /// Called after a successful create with the new workspace id, the
  /// bootstrapped session id (when "create + first workarea" was chosen), or
  /// null (when "just the workspace" was chosen) — the parent closes the modal,
  /// selects the workspace, and (when present) activates the session.
  onCreated: (workspaceId: string, sessionId: string | null) => void;
  onCancel: () => void;
};

export function CreateFromDescription({
  onCreated,
  onCancel,
}: CreateFromDescriptionProps): JSX.Element {
  const queryClient = useQueryClient();

  const [step, setStep] = useState<Step>("describe");
  const [description, setDescription] = useState("");
  // Selected repo ids (editable across steps); seeded by the detector but the
  // user owns the final set.
  const [selectedRepoIds, setSelectedRepoIds] = useState<string[]>([]);
  // Raw cone text per repo id — the controlled `ConePicker` value map. Seeded
  // from `suggestCones`, then user-editable.
  const [coneValues, setConeValues] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);

  const reposQuery = useQuery({
    queryKey: ["repositories"] as const,
    queryFn: () => listRepositories(),
  });
  const repos = useMemo(
    () => reposQuery.data?.repositories ?? [],
    [reposQuery.data],
  );
  const repoById = useMemo(() => {
    const m = new Map<string, Repository>();
    for (const r of repos) m.set(r.id, r);
    return m;
  }, [repos]);

  const selectedRepos = useMemo(
    () =>
      selectedRepoIds
        .map((id) => repoById.get(id))
        .filter((r): r is Repository => r != null),
    [selectedRepoIds, repoById],
  );

  const issueUrl = useMemo(() => detectIssueUrl(description), [description]);

  const derivedName = useMemo(
    () => deriveWorkspaceName(selectedRepos.map((r) => r.name)),
    [selectedRepos],
  );

  // Step 1 → 2: run the deterministic detector to PRE-CHECK the repo subset
  // (editable in step 2; never an auto-pick that bypasses the user).
  function goToRepos(): void {
    setError(null);
    setSelectedRepoIds((prev) =>
      prev.length > 0 ? prev : detectRepos(description, repos),
    );
    setStep("repos");
  }

  function toggleRepo(id: string): void {
    setSelectedRepoIds((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );
  }

  // Step 2 → 3: seed the cone picker with `suggestCones` for each selected
  // repo (a smart default the user then edits). A repo whose suggestion fails
  // (e.g. UNIMPLEMENTED on a suggester-less Core) simply gets an empty seed —
  // the flow continues, manual entry still works.
  const suggestMutation = useMutation({
    mutationFn: async (): Promise<Record<string, string>> => {
      const issueText = description.trim();
      const entries = await Promise.all(
        selectedRepos.map(async (r) => {
          try {
            const paths = await suggestCones(r.id, issueText);
            return [r.id, paths.join("\n")] as const;
          } catch {
            // Degrade gracefully — no suggestion ⇒ inherit/manual.
            return [r.id, ""] as const;
          }
        }),
      );
      return Object.fromEntries(entries);
    },
    onSuccess: (seeded) => {
      // Preserve any cone text the user already edited; only fill blanks.
      setConeValues((prev) => {
        const next = { ...seeded };
        for (const [id, raw] of Object.entries(prev)) {
          if (raw.trim().length > 0) next[id] = raw;
        }
        return next;
      });
      setStep("cones");
    },
    onError: (e) => setError(formatError(e)),
  });

  // The create commit — fired ONLY by an explicit confirmation-slate button.
  // `withWorkarea` distinguishes the §3.8 "create workspace + first workarea"
  // option from "just the workspace, no workarea yet".
  const createMutation = useMutation({
    mutationFn: async (withWorkarea: boolean) => {
      const workspace = await createWorkspace({
        name: derivedName || "New workspace",
        description: description.trim() || undefined,
        repos: selectedRepos.map((r) => ({
          repositoryId: r.id,
          sparseCones: parseConePaths(coneValues[r.id] ?? ""),
        })),
      });
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      let sessionId: string | null = null;
      if (withWorkarea) {
        // Reuse the shared bootstrap (first workarea + session) — the same
        // path `NewWorkspaceModal` uses, so "create + first workarea" lands the
        // user in a ready session.
        const result = await bootstrapWorkspace(workspace.id);
        sessionId = result.sessionId;
        void queryClient.invalidateQueries({
          queryKey: ["workareas", workspace.id],
        });
        void queryClient.invalidateQueries({
          queryKey: ["sessions", result.workareaId],
        });
      }
      return { workspaceId: workspace.id, sessionId };
    },
    onSuccess: ({ workspaceId, sessionId }) =>
      onCreated(workspaceId, sessionId),
    onError: (e) => setError(formatError(e)),
  });

  const busy = suggestMutation.isPending || createMutation.isPending;

  return (
    <div className="space-y-4" data-testid="create-from-description">
      <StepHeader step={step} />

      {error && <p className="text-xs text-err">{error}</p>}

      {step === "describe" && (
        <DescribeStep
          description={description}
          issueUrl={issueUrl}
          onChange={setDescription}
          onCancel={onCancel}
          onNext={goToRepos}
        />
      )}

      {step === "repos" && (
        <ReposStep
          repos={repos}
          loading={reposQuery.isLoading}
          loadError={
            reposQuery.isError ? formatError(reposQuery.error) : null
          }
          selectedRepoIds={selectedRepoIds}
          onToggle={toggleRepo}
          onBack={() => setStep("describe")}
          onNext={() => suggestMutation.mutate()}
          suggesting={suggestMutation.isPending}
        />
      )}

      {step === "cones" && (
        <ConesStep
          repos={selectedRepos}
          values={coneValues}
          onChange={(id, raw) =>
            setConeValues((prev) => ({ ...prev, [id]: raw }))
          }
          onBack={() => setStep("repos")}
          onNext={() => setStep("confirm")}
        />
      )}

      {step === "confirm" && (
        <ConfirmStep
          name={derivedName}
          repos={selectedRepos}
          coneValues={coneValues}
          issueUrl={issueUrl}
          busy={busy}
          onAction={(action) => {
            if (action === "edit") {
              setError(null);
              setStep("repos");
              return;
            }
            setError(null);
            createMutation.mutate(action === "with_workarea");
          }}
          onCancel={onCancel}
        />
      )}
    </div>
  );
}

const STEP_LABELS: Record<Step, string> = {
  describe: "1. Describe the work",
  repos: "2. Confirm the repositories",
  cones: "3. Review suggested cones",
  confirm: "4. Confirm & create",
};

function StepHeader({ step }: { step: Step }): JSX.Element {
  return (
    <p className="text-xs uppercase tracking-wider text-faint">
      {STEP_LABELS[step]}
    </p>
  );
}

function DescribeStep({
  description,
  issueUrl,
  onChange,
  onCancel,
  onNext,
}: {
  description: string;
  issueUrl: string | null;
  onChange: (v: string) => void;
  onCancel: () => void;
  onNext: () => void;
}): JSX.Element {
  return (
    <div className="space-y-3">
      <div>
        <label
          htmlFor="cfd-description"
          className="block text-xs uppercase tracking-wider text-faint mb-1"
        >
          Description or issue link
        </label>
        <textarea
          id="cfd-description"
          aria-label="Description or issue link"
          rows={4}
          value={description}
          onChange={(e) => onChange(e.target.value)}
          placeholder="e.g. add SSO to the API and the iOS app — https://linear.app/acme/issue/LINEAR-123"
          className="w-full rounded-md border border-border-strong bg-background px-2 py-1.5 text-sm text-foreground focus:outline-none focus-visible:ring-2 focus-visible:ring-accent resize-y"
        />
      </div>
      {issueUrl && (
        <p className="flex items-center gap-1.5 text-xs text-faint">
          <Link2 size={12} />
          <span>
            Detected issue link:{" "}
            <span className="font-mono text-foreground">{issueUrl}</span>
          </span>
        </p>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="button"
          variant="primary"
          disabled={description.trim().length === 0}
          onClick={onNext}
        >
          Next <ArrowRight size={13} />
        </Button>
      </div>
    </div>
  );
}

function ReposStep({
  repos,
  loading,
  loadError,
  selectedRepoIds,
  onToggle,
  onBack,
  onNext,
  suggesting,
}: {
  repos: Repository[];
  loading: boolean;
  loadError: string | null;
  selectedRepoIds: string[];
  onToggle: (id: string) => void;
  onBack: () => void;
  onNext: () => void;
  suggesting: boolean;
}): JSX.Element {
  return (
    <div className="space-y-3">
      <p className="text-xs text-faint">
        We detected these repositories from your description. Edit the set —
        nothing is created yet.
      </p>
      {loading && <p className="text-xs text-faint">Loading repositories…</p>}
      {loadError && <p className="text-xs text-err">{loadError}</p>}
      {!loading && repos.length === 0 && (
        <p className="text-xs text-faint">
          No repositories in the registry yet — add some first, then describe
          the work.
        </p>
      )}
      {repos.length > 0 && (
        <ul
          role="group"
          aria-label="Detected repositories"
          className="max-h-44 overflow-y-auto rounded-md border border-border-strong bg-background divide-y divide-border"
        >
          {repos.map((r) => (
            <li key={r.id}>
              <label className="flex items-center gap-2 px-2.5 py-1.5 text-sm text-foreground cursor-pointer hover:bg-surface-2">
                <input
                  type="checkbox"
                  className="accent-accent"
                  checked={selectedRepoIds.includes(r.id)}
                  onChange={() => onToggle(r.id)}
                />
                <span className="truncate font-mono">{r.name}</span>
              </label>
            </li>
          ))}
        </ul>
      )}
      <div className="flex justify-between gap-2">
        <Button type="button" variant="ghost" onClick={onBack}>
          <ArrowLeft size={13} /> Back
        </Button>
        <Button
          type="button"
          variant="primary"
          disabled={selectedRepoIds.length === 0 || suggesting}
          onClick={onNext}
        >
          {suggesting ? (
            <>
              <Loader2 size={13} className="animate-spin" /> Suggesting cones…
            </>
          ) : (
            <>
              Suggest cones <ArrowRight size={13} />
            </>
          )}
        </Button>
      </div>
    </div>
  );
}

function ConesStep({
  repos,
  values,
  onChange,
  onBack,
  onNext,
}: {
  repos: Repository[];
  values: Record<string, string>;
  onChange: (id: string, raw: string) => void;
  onBack: () => void;
  onNext: () => void;
}): JSX.Element {
  return (
    <div className="space-y-3">
      <p className="text-xs text-faint">
        Suggested sparse cones (seeded from the description). Edit them — a
        blank repo inherits the workspace/repo defaults.
      </p>
      <ConePicker repos={repos} values={values} onChange={onChange} />
      <div className="flex justify-between gap-2">
        <Button type="button" variant="ghost" onClick={onBack}>
          <ArrowLeft size={13} /> Back
        </Button>
        <Button type="button" variant="primary" onClick={onNext}>
          Review <ArrowRight size={13} />
        </Button>
      </div>
    </div>
  );
}

function ConfirmStep({
  name,
  repos,
  coneValues,
  issueUrl,
  busy,
  onAction,
  onCancel,
}: {
  name: string;
  repos: Repository[];
  coneValues: Record<string, string>;
  issueUrl: string | null;
  busy: boolean;
  onAction: (action: CreateSlateAction) => void;
  onCancel: () => void;
}): JSX.Element {
  return (
    <div className="space-y-3">
      {/* The §3.8 confirmation slate — a read-only plan summary + the three
          explicit create options. NOTHING is created until a button here. */}
      <div
        className="rounded-md border border-border p-3 space-y-2"
        aria-label="Create plan"
      >
        <div className="text-sm">
          <span className="text-faint">Workspace: </span>
          <span className="font-semibold text-foreground">
            {name || "New workspace"}
          </span>
        </div>
        {issueUrl && (
          <div className="text-xs text-faint">
            From issue{" "}
            <span className="font-mono text-foreground">{issueUrl}</span>
          </div>
        )}
        <ul className="space-y-1">
          {repos.map((r) => {
            const cones = parseConePaths(coneValues[r.id] ?? "");
            return (
              <li key={r.id} className="text-xs text-foreground">
                <span className="font-mono">{r.name}</span>
                <span className="text-faint">
                  {" — "}
                  {cones.length > 0
                    ? `sparse: ${cones.join(", ")}`
                    : "full working tree (inherits defaults)"}
                </span>
              </li>
            );
          })}
        </ul>
      </div>

      <div className="flex flex-col gap-2">
        <Button
          type="button"
          variant="primary"
          disabled={busy}
          onClick={() => onAction("with_workarea")}
        >
          {busy ? (
            <>
              <Loader2 size={13} className="animate-spin" /> Creating…
            </>
          ) : (
            "Create workspace + first workarea"
          )}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={busy}
          onClick={() => onAction("workspace_only")}
        >
          Just the workspace, no workarea
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={busy}
          onClick={() => onAction("edit")}
        >
          Edit repo set / cones
        </Button>
      </div>

      <div className="flex justify-end">
        <Button type="button" variant="ghost" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

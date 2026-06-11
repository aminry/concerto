// Typed wrappers around `Repositories.*` RPCs.
//
// Mirrors `concerto.v1.Repository`. The streaming `Clone` surface
// lives on `clone_repository` (see `client.ts::cloneRepository`) and
// is driven via Tauri's typed event bus, not `callRpc`.

import { callRpc } from "./client";

export type Repository = {
  id: string;
  name: string;
  url: string;
  local_path: string;
  clone_strategy: string;
  default_branch: string;
  last_fetch_at?: [number, number] | null;
  /// The repository's default sparse cone (design/02 §3.2), decoded from
  /// `repositories.cone_defaults_json`. A flat list of forward-slash,
  /// repo-root-relative directory paths inherited by every new workarea.
  /// Always present on the wire (prost serializes `repeated` as `[]`);
  /// optional here so existing `Repository` fixtures stay valid. Readers
  /// treat a missing value as `[]`. The `SparseConeDialog` pre-loads this as
  /// the initial selection. snake_case on the wire (prost-serde).
  cone_defaults?: string[];
};

/// Mirrors `concerto.v1.TreeEntry` (design/02 §3.2) — one entry in a
/// `ListTree` listing. `path` is the full repo-root-relative path (the
/// cone-path a directory row checks); `is_dir` distinguishes a directory (a
/// checkable cone unit) from a file (shown for context, not checkable).
export type TreeEntry = {
  name: string;
  is_dir: boolean;
  path: string;
};

export type ListTreeResponse = {
  entries: TreeEntry[];
};

export type ListRepositoriesResponse = {
  repositories: Repository[];
};

/// Mirrors `concerto.v1.SizeReport` (Task 301, FROZEN). The pre-clone
/// probe the add-repo dialog runs against a URL to drive the design/02
/// §3.5 size→strategy recommendation. `recommended_strategy` is one of
/// `full | blobless` — treeless is NEVER recommended (design/02 §12 R-1).
/// `recommend_sparse` is the >10 GB-tier cone-picker hint. `uint64` lands
/// as a JS `number` under the prost-serde shim (same convention as
/// `ConeStats`/`CloneProgress`). A failed probe (private/offline repo)
/// surfaces as a `CoreClientError` the caller falls back from.
export type SizeReport = {
  size_bytes: number;
  object_count: number;
  branch_count: number;
  recommended_strategy: string;
  recommend_sparse: boolean;
};

/// `Repositories.EstimateRepoSize` — pre-clone remote probe (Task 301).
/// Runs `git ls-remote` + a cheap object probe on the Core and returns the
/// design/02 §3.5 recommendation. Throws a `CoreClientError` when the
/// remote can't be reached (private/offline) — the caller treats that as
/// "no recommendation, manual pick".
export async function estimateRepoSize(url: string): Promise<SizeReport> {
  return callRpc<{ url: string }, SizeReport>(
    "Repositories.EstimateRepoSize",
    { url },
  );
}

/// `Repositories.ListRepositories` — lists ALL repositories in the global
/// registry (the Project layer was collapsed away; repos are no longer
/// project-scoped).
export async function listRepositories(): Promise<ListRepositoriesResponse> {
  return callRpc<Record<string, never>, ListRepositoriesResponse>(
    "Repositories.ListRepositories",
    {},
  );
}

/// `Repositories.ListTree` (design/02 §3.2) — list the IMMEDIATE
/// (non-recursive) children of `path` at `gitRef`. Backs the lazy repo-tree
/// picker: each directory's children are fetched only when it is expanded.
/// `path` is repo-root-relative (`""` = root); `gitRef` empty ⇒ the repo's
/// default branch / HEAD. Entries are returned trees-first.
export async function listTree(
  repositoryId: string,
  path = "",
  gitRef = "",
): Promise<ListTreeResponse> {
  return callRpc<
    { repository_id: string; path: string; git_ref: string },
    ListTreeResponse
  >("Repositories.ListTree", {
    repository_id: repositoryId,
    path,
    git_ref: gitRef,
  });
}

/// `Repositories.SetRepoConeDefaults` (design/02 §3.2) — set the
/// repository's default sparse cone AND propagate it to every existing
/// workarea of the repo. `coneDefaults` are forward-slash, repo-root-relative
/// directory paths; `[]` clears the default. Returns the updated
/// `Repository` (whose `cone_defaults` reflects the applied set). A bad path
/// → INVALID_ARGUMENT surfaced as a `CoreClientError`, before anything is
/// persisted.
export async function setRepoConeDefaults(
  repositoryId: string,
  coneDefaults: string[],
): Promise<Repository> {
  return callRpc<{ repository_id: string; cone_defaults: string[] }, Repository>(
    "Repositories.SetRepoConeDefaults",
    {
      repository_id: repositoryId,
      cone_defaults: coneDefaults,
    },
  );
}

/// `Repositories.SuggestCones` (Task 411 backend, design/08 §3.8) — the
/// plan-mode cone suggestion the create-workspace-from-description flow calls:
/// given an added `repositoryId` and the parsed issue/description `issueText`,
/// the Repo Mgr delegates to the injected Maestro-backed `ConeSuggester` and
/// returns a suggested sparse cone (forward-slash, repo-root-relative directory
/// prefixes — the same shape `SetCones`/the cone picker consume). The suggested
/// set SEEDS the existing `ConePicker` so the user edits from a smart default;
/// it is never applied silently (R-2). When the Core was built without an
/// injected suggester the RPC returns UNIMPLEMENTED — surfaced as a
/// `CoreClientError` the caller falls back from (empty picker, manual entry).
export async function suggestCones(
  repositoryId: string,
  issueText: string,
): Promise<string[]> {
  const res = await callRpc<
    { repository_id: string; issue_text: string },
    { cone_paths: string[] }
  >("Repositories.SuggestCones", {
    repository_id: repositoryId,
    issue_text: issueText,
  });
  return res.cone_paths ?? [];
}

/// The user-selectable clone strategies (Task 301). Treeless is omitted by
/// design — it is never offered in the UI (design/02 §12 R-1). "Blobless +
/// Sparse" is `cloneStrategy: "blobless"` + `withSparse: true`, so the two
/// knobs below fully express the three picker choices.
export type CloneStrategy = "full" | "blobless";

/// Register a repository in the global registry. Two variants:
///   - `{ url, ... }` clones a remote into the shared pool.
///   - `{ localPath, name }` ADOPTS an existing on-disk git repo in place
///     (non-destructive). Exactly one of `url` / `localPath` is set.
export type AddRepositoryInput =
  | {
      name: string;
      url: string;
      localPath?: undefined;
      defaultBranch?: string;
      /// Empty/omitted → Full on the Core (preserves the original behavior).
      cloneStrategy?: CloneStrategy;
      /// When true, clone `--sparse --no-checkout` so the cone picker can size
      /// the worktree afterwards (the ">10 GB → Blobless + Sparse" tier).
      withSparse?: boolean;
    }
  | {
      name: string;
      localPath: string;
      url?: undefined;
    };

export async function addRepository(
  input: AddRepositoryInput,
): Promise<Repository> {
  // The local-folder variant adopts an existing repo in place, so the
  // clone-strategy knobs do not apply; the url variant clones.
  const wire =
    input.localPath !== undefined
      ? {
          name: input.name,
          url: "",
          default_branch: "",
          clone_strategy: "",
          with_sparse: false,
          local_path: input.localPath,
        }
      : {
          name: input.name,
          url: input.url,
          default_branch: input.defaultBranch ?? "",
          clone_strategy: input.cloneStrategy ?? "",
          with_sparse: input.withSparse ?? false,
          local_path: "",
        };
  return callRpc<typeof wire, Repository>(
    "Repositories.AddRepository",
    wire,
  );
}

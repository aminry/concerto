// Typed wrappers around `Repositories.*` RPCs.
//
// Mirrors `concerto.v1.Repository`. The streaming `Clone` surface
// lives on `clone_repository` (see `client.ts::cloneRepository`) and
// is driven via Tauri's typed event bus, not `callRpc`.

import { callRpc } from "./client";

export type Repository = {
  id: string;
  project_id: string;
  name: string;
  url: string;
  local_path: string;
  clone_strategy: string;
  default_branch: string;
  last_fetch_at?: [number, number] | null;
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

export async function listRepositories(
  projectId: string,
): Promise<ListRepositoriesResponse> {
  return callRpc<{ project_id: string }, ListRepositoriesResponse>(
    "Repositories.ListByProject",
    { project_id: projectId },
  );
}

/// The user-selectable clone strategies (Task 301). Treeless is omitted by
/// design — it is never offered in the UI (design/02 §12 R-1). "Blobless +
/// Sparse" is `cloneStrategy: "blobless"` + `withSparse: true`, so the two
/// knobs below fully express the three picker choices.
export type CloneStrategy = "full" | "blobless";

export async function addRepository(input: {
  projectId: string;
  name: string;
  url: string;
  defaultBranch?: string;
  /// Empty/omitted → Full on the Core (preserves the original behavior).
  cloneStrategy?: CloneStrategy;
  /// When true, clone `--sparse --no-checkout` so the cone picker can size
  /// the worktree afterwards (the ">10 GB → Blobless + Sparse" tier).
  withSparse?: boolean;
}): Promise<Repository> {
  return callRpc<
    {
      project_id: string;
      name: string;
      url: string;
      default_branch: string;
      clone_strategy: string;
      with_sparse: boolean;
    },
    Repository
  >("Repositories.AddRepository", {
    project_id: input.projectId,
    name: input.name,
    url: input.url,
    default_branch: input.defaultBranch ?? "",
    clone_strategy: input.cloneStrategy ?? "",
    with_sparse: input.withSparse ?? false,
  });
}

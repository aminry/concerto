// Typed wrappers around `Workareas.*` RPCs.
//
// Mirrors `concerto.v1.Workarea`. As with the other proto mirrors in
// this directory (see `workspaces.ts`), prost-serde keeps snake_case
// field names; timestamps land as `[seconds, nanos]` tuples or null.
//
// ── Multi-repo workarea surface ──────────────────────────────────────
// A V1.0 workspace declares 1..N repos and every workarea on it materializes
// one worktree per declared repo (design/03 §6.2), recorded in the
// `workarea_repos` junction. The per-repo diff RPC
// `GetWorkareaRepoDiff(workarea_id, repository_id)` accepts exactly those
// attached repository ids and rejects any other ("not attached to workarea").
//
// `Workareas.ListWorkareaRepos` returns precisely that attached set (full
// `Repository` rows). It is the workarea-scoped repo source the Diff panel
// uses — see `useWorkareaRepos`. The global `Repositories.ListRepositories`
// is the unscoped registry (every repo across all workspaces) and MUST NOT be
// used for a workarea's repo list: it would offer repos the workarea never
// materialized, which `GetWorkareaRepoDiff` then rejects.

import { callRpc } from "./client";
import { setCones } from "./cones";
import type { Repository } from "./repositories";

export type Workarea = {
  id: string;
  workspace_id: string;
  composer_name: string;
  branch_name: string;
  worktree_root: string;
  // status ∈ { created | active | running | awaiting | paused | finished | partial | archived | crashed }
  // (Task 307 widened the set with `finished` + `partial`.)
  status: string;
  permission_mode?: number | null;
  created_at?: [number, number] | null;
  last_activity_at?: [number, number] | null;
  archived_at?: [number, number] | null;
};

export type ListWorkareasResponse = {
  workareas: Workarea[];
};

export async function listWorkareas(
  workspaceId: string,
  includeArchived = false,
): Promise<ListWorkareasResponse> {
  return callRpc<
    { workspace_id: string; include_archived: boolean },
    ListWorkareasResponse
  >("Workareas.ListWorkareas", {
    workspace_id: workspaceId,
    include_archived: includeArchived,
  });
}

export async function getWorkarea(id: string): Promise<Workarea> {
  return callRpc<{ id: string }, Workarea>("Workareas.GetWorkarea", { id });
}

export type ListWorkareaReposResponse = {
  repositories: Repository[];
};

/// `Workareas.ListWorkareaRepos` — the repositories actually attached
/// (materialized) to a workarea, read from the `workarea_repos` junction.
/// This is the workarea-scoped repo list the Diff panel renders: every repo
/// returned is one `GetWorkareaRepoDiff` will accept. A repo-less / unknown
/// workarea yields an empty list. Distinct from the unscoped global
/// `Repositories.ListRepositories`.
export async function listWorkareaRepos(
  workareaId: string,
): Promise<ListWorkareaReposResponse> {
  return callRpc<{ workarea_id: string }, ListWorkareaReposResponse>(
    "Workareas.ListWorkareaRepos",
    { workarea_id: workareaId },
  );
}

/// A per-repo sparse cone chosen at workarea-create time. `cone_paths`
/// empty ⇒ "use the inherited workspace/repo defaults" — the cone is NOT
/// sent for that repo, so the Core applies the three-layer inherited
/// defaults it seeds at create (Task 302/306).
export type WorkareaRepoCones = {
  repository_id: string;
  cone_paths: string[];
};

/// Options for `createWorkarea`. `cones` threads the per-repo sparse-cone
/// picker choices (Task 322).
export type CreateWorkareaOptions = {
  permissionMode?: number;
  /// Per-repo cones chosen in the picker. Applied AFTER the workarea is
  /// created — see the note below.
  cones?: WorkareaRepoCones[];
};

/// Create a workarea, then apply any chosen per-repo sparse cones.
///
/// ── Why cones are applied post-create (Task 322 drift) ───────────────
/// The task contract assumed 302/306/307 added a `cones` field to
/// `CreateWorkareaRequest`. They did NOT: `CreateWorkareaRequest` is still
/// `{ workspace_id, permission_mode }` on `main`, and adding a field is a
/// Rust/proto change this task is forbidden from making. The FROZEN path
/// to set a per-(workarea, repo) cone is `Repositories.SetCones`
/// (Task 302), which is inherently POST-create (it needs a `workarea_id`).
///
/// So: create the workarea, then for each repo with a non-empty cone fire
/// `SetCones`. A repo with an empty cone is skipped entirely so the Core's
/// inherited-defaults seed (302's resolver) stands. A `SetCones` rejection
/// (bad path → INVALID_ARGUMENT) propagates; the picker validates each
/// cone via `EstimateConeSize` before submit to keep that rare.
export async function createWorkarea(
  workspaceId: string,
  options?: CreateWorkareaOptions,
): Promise<Workarea> {
  const workarea = await callRpc<
    { workspace_id: string; permission_mode?: number },
    Workarea
  >("Workareas.CreateWorkarea", {
    workspace_id: workspaceId,
    permission_mode: options?.permissionMode,
  });

  const cones = options?.cones ?? [];
  for (const entry of cones) {
    if (entry.cone_paths.length === 0) continue; // empty ⇒ inherit defaults
    await setCones(workarea.id, entry.repository_id, entry.cone_paths);
  }

  return workarea;
}

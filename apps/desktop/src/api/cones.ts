// Typed wrappers around the sparse-cone RPCs the cone picker (Task 322)
// drives.
//
// Two FROZEN upstream surfaces are mirrored here:
//
//   - `Repositories.EstimateConeSize` (Task 305) — the post-clone,
//     per-cone, git-index-read telemetry probe. Request carries a
//     `repository_id` + a list of `cone_paths` (forward-slash,
//     repo-root-relative directory prefixes); the response is a
//     `ConeStats` `{ file_count, disk_size_bytes }` (proto field numbers
//     1/2, FROZEN by PHASE3_PLANNING §4.6).
//
//   - `Repositories.SetCones` (Task 302) — the per-(workarea, repo) cone
//     setter. Applies the cone to the on-disk worktree (cone-mode +
//     `--sparse-index` always) and persists it to
//     `workarea_repos.sparse_cones_json`. A path absent from the repo's
//     HEAD tree is rejected with INVALID_ARGUMENT and nothing is
//     half-applied (design/02 §8) — the picker surfaces that inline.
//
// `uint64` lands as a JS `number` under the prost-serde shim (the same
// convention `CloneProgress`/`SizeReport` use in `client.ts`/`repositories.ts`;
// confirmed against `runtime.test.ts`). Cone byte totals are an
// order-of-magnitude estimate (a blobless clone's not-yet-fetched blobs
// read as size 0), so the UI labels the size as an estimate.

import { callRpc } from "./client";

/// Mirrors `concerto.v1.ConeStats` (Task 305, FROZEN by PHASE3_PLANNING
/// §4.6). `disk_size_bytes` is a lower bound for a blobless clone — label
/// it as an estimate in the UI. `file_count` is exact regardless of fetch
/// state (it counts file entries in the git index/sparse-index).
export type ConeStats = {
  file_count: number;
  disk_size_bytes: number;
};

/// `Repositories.EstimateConeSize` — read the git index and return the
/// `(file_count, disk_size_bytes)` the given cone would materialize. An
/// empty `conePaths` falls back to the repository's `cone_defaults_json`,
/// then (when that is also empty) counts every tracked file.
export async function estimateConeSize(
  repositoryId: string,
  conePaths: string[],
): Promise<ConeStats> {
  return callRpc<
    { repository_id: string; cone_paths: string[] },
    ConeStats
  >("Repositories.EstimateConeSize", {
    repository_id: repositoryId,
    cone_paths: conePaths,
  });
}

/// `Repositories.SetCones` — set the per-(workarea, repo) sparse cone.
/// Echoes back the cone paths that are now active (the set the worktree
/// was materialized to). A bad path → INVALID_ARGUMENT surfaced as a
/// `CoreClientError` `{kind,message}` the caller renders inline.
export async function setCones(
  workareaId: string,
  repositoryId: string,
  conePaths: string[],
): Promise<{ cone_paths: string[] }> {
  return callRpc<
    { workarea_id: string; repository_id: string; cone_paths: string[] },
    { cone_paths: string[] }
  >("Repositories.SetCones", {
    workarea_id: workareaId,
    repository_id: repositoryId,
    cone_paths: conePaths,
  });
}

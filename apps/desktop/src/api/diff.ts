// Typed wrapper around `Workareas.GetWorkareaRepoDiff`.
//
// Mirrors `concerto.v1.DiffPayload` / `FileDiff` / `DiffHunk` /
// `DiffKind` from `crates/proto/proto/concerto/v1/workareas.proto`
// (Task 29). Prost-serde keeps the proto's snake_case field names and
// emits `DiffKind` as the bare enum string (e.g. `"DIFF_KIND_ADDED"`).

import { callRpc } from "./client";

/// Mirror of `concerto.v1.DiffKind`. The values match the proto's
/// uppercase identifiers because prost-serde renders the variants as
/// their textual names on the wire.
export type DiffKind =
  | "DIFF_KIND_UNSPECIFIED"
  | "DIFF_KIND_ADDED"
  | "DIFF_KIND_DELETED"
  | "DIFF_KIND_MODIFIED"
  | "DIFF_KIND_RENAMED";

/// Mirror of `concerto.v1.DiffHunk`. `body` is the unified-diff text
/// for the hunk, joined with `\n`; the four range fields carry the
/// hunk-header information.
export type DiffHunk = {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  body: string;
};

/// Mirror of `concerto.v1.FileDiff`.
export type FileDiff = {
  path: string;
  kind: DiffKind;
  old_path?: string | null;
  hunks: DiffHunk[];
};

/// Mirror of `concerto.v1.DiffPayload`.
export type DiffPayload = {
  files: FileDiff[];
};

export async function getWorkareaRepoDiff(
  workareaId: string,
  repositoryId: string,
): Promise<DiffPayload> {
  return callRpc<
    { workarea_id: string; repository_id: string },
    DiffPayload
  >("Workareas.GetWorkareaRepoDiff", {
    workarea_id: workareaId,
    repository_id: repositoryId,
  });
}

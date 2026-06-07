// Typed wrappers + wire mirrors for the VCS / coordinated-merge surface
// (Task 324). Every shape here mirrors an UPSTREAM-FROZEN wire contract;
// this file is the renderer-side consumer (`design/15 §3.4`). It defines
// no new wire — each mirror notes the task that froze it.
//
// ── Where the wire shapes come from ──────────────────────────────────
// - `PullRequest` (fields 1–14): `vcs.proto`, FROZEN at Task 45.
//   `merge_order` (15) + `external_id` (16) + `repository_full_name` (17):
//   Task 319 (migration 0014).
// - `CheckRun`: `vcs.proto`, FROZEN at Task 45.
// - `ReviewThread` / `ReviewThreadComment` / `Deployment`: `vcs.proto`,
//   FROZEN at Task 316.
// - `GetWorkareaPrSetResponse`: `workareas.proto`, Task 45 (ordering
//   `(merge_order, pr_number)` set by Task 319).
// - `SetMergeOrderRequest`: `workareas.proto`, Task 319.
// - `MergePlan` / `MergeStep` / `MergeProgress` (+ inner) / `RevertReport`
//   / `RevertStep` / `FailureKind` / `RevertOutcome`: `workareas.proto`,
//   FROZEN at Task 320.
// - The `checks.<wa>.<repo>` + `pr_set.events` opaque JSON frames are
//   parsed client-side (PHASE3_PLANNING §2 — NO proto Event oneof arm);
//   their schemas are FROZEN by Tasks 316 / 320 respectively and mirrored
//   in `parseChecksFrame` / `parsePrSetFrame` below.
//
// ── Dispatch (Task 218 `CoreClient`) ─────────────────────────────────
// Reads/mutations go through `callRpc(<Service>.<Rpc>, payload)`. The
// `MergeWorkareaPrSet` server-stream is consumed through the
// `pr_set.events.<workarea_id>` pub/sub subject (the SAME opaque carrier
// the rest of the streams surface uses), which IS reachable via the
// generic `Streams.Subscribe` bridge — see `useMergeProgress`. 320's
// handoff: "the stream is the source of truth for the merging client;
// `pr_set.events` is for everyone else"; with no `src-tauri` streaming
// command for the merge RPC (this task is `web-ts`, no Rust), the renderer
// rides the `pr_set.events` projection of the same lifecycle. (See the
// Handoff Notes drift entry — the dedicated merge streaming command is the
// Rust-shell follow-on, mirroring how Task 322 left `EstimateConeSize` /
// `SetCones` un-dispatched in `rpc.rs`.)

import { callRpc } from "./client";

// ─── PullRequest (vcs.proto 1–14, Task 45; 15–17 Task 319/316) ───────

/// PR lifecycle state for the GitHub provider (`vcs.proto` PullRequest.state).
export type PrState = "open" | "closed" | "merged" | "draft";

/// Mirror of `concerto.v1.PullRequest`. Prost-serde keeps snake_case on the
/// wire; `int64` fields (`pr_number`, `created_at`, `updated_at`,
/// `merge_order`) land as `number` (JSON-safe for the values GitHub emits).
export type PullRequest = {
  id: string;
  workarea_id: string;
  repository_id: string;
  provider: string;
  pr_number: number;
  base_ref: string;
  head_ref: string;
  state: PrState | string;
  title: string;
  body: string;
  url: string;
  head_sha: string;
  created_at: number;
  updated_at: number;
  /// Task 319 — position within the workarea's merge plan; default =
  /// insertion order. Re-orderable via `setMergeOrder`.
  merge_order: number;
  /// Task 319 — the PR's GraphQL node id (empty for pre-octocrab rows).
  external_id?: string;
  /// Task 319 — the `owner/repo` string (empty for pre-octocrab rows).
  repository_full_name?: string;
};

// ─── CheckRun (vcs.proto, Task 45) ───────────────────────────────────

export type CheckStatus = "queued" | "in_progress" | "completed";

export type CheckConclusion =
  | "success"
  | "failure"
  | "neutral"
  | "cancelled"
  | "timed_out"
  | "action_required"
  | "stale"
  | "skipped";

/// Mirror of `concerto.v1.CheckRun`. `conclusion` is only meaningful once
/// `status === "completed"`.
export type CheckRun = {
  name: string;
  status: CheckStatus | string;
  conclusion: CheckConclusion | string;
  details_url: string;
};

export type GetChecksResponse = {
  checks: CheckRun[];
};

// ─── Review threads / deployments (vcs.proto, Task 316) ──────────────

/// Mirror of `concerto.v1.ReviewThreadComment`. NOTE: 316's handoff froze
/// the value type to comment BODIES only — `author` is emitted empty on the
/// `ListReviewThreads` RPC for now (widening it is a future "revise 313
/// value types" task). The `checks.<wa>.<repo>` opaque frame carries bare
/// body strings (see `ChecksThreadFrame`).
export type ReviewThreadComment = {
  author: string;
  body: string;
};

/// Mirror of `concerto.v1.ReviewThread`.
export type ReviewThread = {
  id: string;
  resolved: boolean;
  path: string;
  comments: ReviewThreadComment[];
};

export type ListReviewThreadsResponse = {
  threads: ReviewThread[];
};

// ─── Coordinated-merge wire (workareas.proto, Task 319/320) ──────────

export type GetWorkareaPrSetResponse = {
  pull_requests: PullRequest[];
};

/// Mirror of `concerto.v1.MergeStep` (Task 320). 1-based positional
/// `step`/`total`; `merge_order` is the raw (possibly non-contiguous) order.
export type MergeStep = {
  step: number;
  total: number;
  repository_id: string;
  repository_full_name: string;
  pr_number: number;
  head_sha: string;
  merge_order: number;
  state: string;
};

/// Mirror of `concerto.v1.MergePlan` (Task 320).
export type MergePlan = {
  workarea_id: string;
  steps: MergeStep[];
};

/// Mirror of `concerto.v1.FailureKind` (Task 320). Prost-serde renders proto
/// enums as their UPPER_SNAKE textual names on the wire.
export type FailureKind =
  | "FAILURE_KIND_UNSPECIFIED"
  | "FAILURE_KIND_CHECKS_FAILED"
  | "FAILURE_KIND_CHECKS_TIMEOUT"
  | "FAILURE_KIND_MERGE_CONFLICT"
  | "FAILURE_KIND_MERGE_REJECTED";

/// Mirror of `concerto.v1.RevertOutcome` (Task 320).
export type RevertOutcome =
  | "REVERT_OUTCOME_UNSPECIFIED"
  | "REVERT_OUTCOME_REVERTED"
  | "REVERT_OUTCOME_SKIPPED"
  | "REVERT_OUTCOME_FAILED";

/// Mirror of `concerto.v1.RevertStep` (Task 320).
export type RevertStep = {
  repository_full_name: string;
  pr_number: number;
  outcome: RevertOutcome | string;
  detail: string;
};

/// Mirror of `concerto.v1.RevertReport` (Task 320). The set is walked in
/// REVERSE `merge_order`; only merged members are revertible.
export type RevertReport = {
  workarea_id: string;
  steps: RevertStep[];
};

// MergeProgress oneof inner messages (Task 320). The `MergeWorkareaPrSet`
// server-stream emits a sequence of these; this task surfaces the same
// lifecycle via the `pr_set.events` opaque frame (see `parsePrSetFrame`),
// then normalizes both onto the `MergeProgress` shape below.

export type MergeStepStarted = {
  step: number;
  total: number;
  repository_full_name: string;
  pr_number: number;
};

export type MergeStepCompleted = {
  step: number;
  total: number;
  merge_sha: string;
};

export type MergeStepFailed = {
  step: number;
  total: number;
  reason: string;
  kind: FailureKind | string;
};

export type MergeSetMerged = { total: number };

export type MergeSetPaused = {
  paused_at_step: number;
  total: number;
  reason: string;
};

/// Normalized mirror of `concerto.v1.MergeProgress` (Task 320). The proto is
/// a `oneof event { ... }`; we model it as a tagged union keyed by the
/// variant name so the UI switches on `frame.kind`. `set_paused` is the
/// "Step N of M failed — auto-revert?" surface.
export type MergeProgress =
  | { kind: "step_started"; data: MergeStepStarted }
  | { kind: "step_completed"; data: MergeStepCompleted }
  | { kind: "step_failed"; data: MergeStepFailed }
  | { kind: "set_merged"; data: MergeSetMerged }
  | { kind: "set_paused"; data: MergeSetPaused };

// ─── Vcs.* bindings (Task 45 / 316) ──────────────────────────────────

/// `Vcs.GetPullRequest` — read + upsert the live PR for `(repo, pr_number)`.
export async function getPullRequest(
  repositoryId: string,
  prNumber: number,
): Promise<PullRequest> {
  return callRpc<{ repository_id: string; pr_number: number }, PullRequest>(
    "Vcs.GetPullRequest",
    { repository_id: repositoryId, pr_number: prNumber },
  );
}

/// `Vcs.CreatePullRequest` — `head` is the agent-pushed branch; `base` empty
/// ⇒ the repo default branch (the server does NOT push in V0.1).
export async function createPullRequest(input: {
  workareaId: string;
  repositoryId: string;
  base?: string;
  head: string;
  title: string;
  body?: string;
}): Promise<PullRequest> {
  return callRpc<
    {
      workarea_id: string;
      repository_id: string;
      base: string;
      head: string;
      title: string;
      body: string;
    },
    PullRequest
  >("Vcs.CreatePullRequest", {
    workarea_id: input.workareaId,
    repository_id: input.repositoryId,
    base: input.base ?? "",
    head: input.head,
    title: input.title,
    body: input.body ?? "",
  });
}

/// `Vcs.MergePullRequest` — `method` ∈ `merge|squash|rebase` (empty ⇒
/// `merge`). Returns `google.protobuf.Empty` (the shell maps to `null`).
export async function mergePullRequest(
  repositoryId: string,
  prNumber: number,
  method: "merge" | "squash" | "rebase" = "merge",
): Promise<void> {
  await callRpc<
    { repository_id: string; pr_number: number; method: string },
    null
  >("Vcs.MergePullRequest", {
    repository_id: repositoryId,
    pr_number: prNumber,
    method,
  });
}

/// `Vcs.GetChecks` — the check runs for `sha` on `repositoryId`.
export async function getChecks(
  repositoryId: string,
  sha: string,
): Promise<GetChecksResponse> {
  return callRpc<{ repository_id: string; sha: string }, GetChecksResponse>(
    "Vcs.GetChecks",
    { repository_id: repositoryId, sha },
  );
}

// ─── Workareas.* PR-set bindings (Task 319 / 320) ────────────────────
//
// The PR-set RPCs take a `WorkareaId { value }` wrapper (mirrors
// `GetWorkarea`'s `{ id }` for the legacy ones, but the proto wraps the id
// as `{ value }` — see `workareas.proto::WorkareaId`).

/// `Workareas.GetWorkareaPrSet` — the implicit PR set, ordered
/// `(merge_order, pr_number)` (Task 319).
export async function getWorkareaPrSet(
  workareaId: string,
): Promise<GetWorkareaPrSetResponse> {
  return callRpc<{ value: string }, GetWorkareaPrSetResponse>(
    "Workareas.GetWorkareaPrSet",
    { value: workareaId },
  );
}

/// `Workareas.SetMergeOrder` — write one PR's `merge_order`; returns the
/// re-ordered set (Task 319). The drag-to-reorder write path.
export async function setMergeOrder(input: {
  workareaId: string;
  repositoryId: string;
  mergeOrder: number;
}): Promise<GetWorkareaPrSetResponse> {
  return callRpc<
    { workarea_id: string; repository_id: string; merge_order: number },
    GetWorkareaPrSetResponse
  >("Workareas.SetMergeOrder", {
    workarea_id: input.workareaId,
    repository_id: input.repositoryId,
    merge_order: input.mergeOrder,
  });
}

/// `Workareas.GetWorkareaMergePlan` — the read-only ordered `(repo, PR)`
/// preview (Task 320).
export async function getWorkareaMergePlan(
  workareaId: string,
): Promise<MergePlan> {
  return callRpc<{ value: string }, MergePlan>(
    "Workareas.GetWorkareaMergePlan",
    { value: workareaId },
  );
}

/// `Workareas.RevertWorkareaPrSet` — coordinated revert (reverse
/// `merge_order`); `hardReset` opts into hard-reset over revert-commit
/// (Task 320).
export async function revertWorkareaPrSet(
  workareaId: string,
  hardReset = false,
): Promise<RevertReport> {
  return callRpc<
    { workarea_id: string; hard_reset: boolean },
    RevertReport
  >("Workareas.RevertWorkareaPrSet", {
    workarea_id: workareaId,
    hard_reset: hardReset,
  });
}

/// Trigger `Workareas.MergeWorkareaPrSet`. The RPC is a server-stream on the
/// wire; the renderer consumes its lifecycle through the
/// `pr_set.events.<workarea_id>` pub/sub subject (see `useMergeProgress`),
/// so this fires the start signal and the live frames arrive over the bus.
///
/// In production the dedicated merge streaming command (the Rust-shell
/// follow-on noted in this task's Handoff) drives the loop; the trigger is
/// dispatched through the generic `callRpc` path here so the renderer never
/// speaks gRPC. `method` ∈ `merge|squash|rebase`.
export async function mergeWorkareaPrSet(input: {
  workareaId: string;
  method?: "merge" | "squash" | "rebase";
  timeoutSecs?: number;
  allowFailingChecks?: boolean;
}): Promise<void> {
  await callRpc<
    {
      workarea_id: string;
      method: string;
      timeout_secs: number;
      allow_failing_checks: boolean;
    },
    unknown
  >("Workareas.MergeWorkareaPrSet", {
    workarea_id: input.workareaId,
    method: input.method ?? "merge",
    timeout_secs: input.timeoutSecs ?? 0,
    allow_failing_checks: input.allowFailingChecks ?? false,
  });
}

// ─── Opaque-frame parsing (PHASE3_PLANNING §2 — client-owned) ─────────
//
// Both `checks.<wa>.<repo>` (Task 316) and `pr_set.events` (Task 320) ride
// the non-oneof `Event.checks_opaque = 17` field, which is proto `bytes`.
// Prost-serde serializes `bytes` as a JSON array of u8 (the SAME hop the
// `session_io.data` chunk uses — see `sessions.ts`). The renderer therefore
// receives `{ offset, at, checks_opaque: number[] }`; decode the byte array
// → UTF-8 → JSON.parse to recover the FROZEN frame.

/// The opaque `Event` shape on `checks.*` / `pr_set.events`. `checks_opaque`
/// is the byte array carrying the JSON frame; `body` is absent (the producer
/// builds a body-LESS Event).
export type OpaqueEvent = {
  offset?: number;
  at?: [number, number] | null;
  checks_opaque?: number[] | null;
};

/// Decode `Event.checks_opaque` (a u8 array) into its parsed JSON frame, or
/// `null` when absent / malformed. Tolerates a base64 string too, in case a
/// future serde config flips `bytes` to base64 (defensive; the current wire
/// is a number array).
export function decodeOpaqueFrame(ev: OpaqueEvent): unknown {
  const raw = ev?.checks_opaque;
  if (raw == null) return null;
  let text: string;
  try {
    if (typeof raw === "string") {
      // base64 fallback (not the current wire shape).
      text =
        typeof atob === "function"
          ? atob(raw)
          : Buffer.from(raw, "base64").toString("utf8");
    } else {
      text = new TextDecoder().decode(Uint8Array.from(raw));
    }
    return JSON.parse(text);
  } catch {
    return null;
  }
}

// --- checks.<wa>.<repo> frame schemas (Task 316, FROZEN) -------------
// `{ kind, workarea_id, repository_id, entity }`.

export type ChecksFrameKind =
  | "check_run_updated"
  | "thread_updated"
  | "deployment_updated";

export type ChecksCheckRunFrame = {
  kind: "check_run_updated";
  workarea_id: string;
  repository_id: string;
  entity: {
    sha: string;
    runs: CheckRun[];
  };
};

export type ChecksThreadFrame = {
  kind: "thread_updated";
  workarea_id: string;
  repository_id: string;
  entity: {
    id: string;
    resolved: boolean;
    path: string | null;
    /// 316 froze the frame's `comments` to bare body strings (no author).
    comments: string[];
  };
};

export type ChecksDeploymentFrame = {
  kind: "deployment_updated";
  workarea_id: string;
  repository_id: string;
  entity: {
    ref: string;
    deployments: Array<{
      id: string;
      environment: string;
      state: string;
      ref: string;
    }>;
  };
};

export type ChecksFrame =
  | ChecksCheckRunFrame
  | ChecksThreadFrame
  | ChecksDeploymentFrame;

/// Parse a decoded `checks.<wa>.<repo>` frame. Returns `null` for anything
/// that isn't one of the three FROZEN kinds.
export function parseChecksFrame(value: unknown): ChecksFrame | null {
  if (!value || typeof value !== "object") return null;
  const f = value as { kind?: unknown };
  if (
    f.kind === "check_run_updated" ||
    f.kind === "thread_updated" ||
    f.kind === "deployment_updated"
  ) {
    return value as ChecksFrame;
  }
  return null;
}

// --- pr_set.events frame schema (Task 320, FROZEN) -------------------
// `{ kind: merge_step_completed|merge_failed_step|merged|reverted, ... }`.

export type PrSetFrame =
  | {
      kind: "merge_step_completed";
      workarea_id: string;
      step: number;
      total: number;
      repository_full_name: string;
      pr_number: number;
      merge_sha: string;
    }
  | {
      kind: "merge_failed_step";
      workarea_id: string;
      step: number;
      total: number;
      reason: string;
    }
  | { kind: "merged"; workarea_id: string; total: number }
  | {
      kind: "reverted";
      workarea_id: string;
      repository_full_name: string;
      pr_number: number;
    };

/// Parse a decoded `pr_set.events` frame.
export function parsePrSetFrame(value: unknown): PrSetFrame | null {
  if (!value || typeof value !== "object") return null;
  const f = value as { kind?: unknown };
  if (
    f.kind === "merge_step_completed" ||
    f.kind === "merge_failed_step" ||
    f.kind === "merged" ||
    f.kind === "reverted"
  ) {
    return value as PrSetFrame;
  }
  return null;
}

/// Normalize a `pr_set.events` frame onto the `MergeProgress` shape the merge
/// UI renders (the `pr_set.events` projection of the `MergeWorkareaPrSet`
/// stream — 320's "for everyone else" lifecycle). `reverted` is not a
/// `MergeProgress` arm (the revert surfaces via `RevertReport`); it returns
/// `null` so the merge view ignores it.
export function prSetFrameToProgress(frame: PrSetFrame): MergeProgress | null {
  switch (frame.kind) {
    case "merge_step_completed":
      return {
        kind: "step_completed",
        data: {
          step: frame.step,
          total: frame.total,
          merge_sha: frame.merge_sha,
        },
      };
    case "merge_failed_step":
      return {
        kind: "set_paused",
        data: {
          paused_at_step: frame.step,
          total: frame.total,
          reason: frame.reason,
        },
      };
    case "merged":
      return { kind: "set_merged", data: { total: frame.total } };
    case "reverted":
      return null;
  }
}

// ─── Check-run colour banding (design/15 §3.4) ───────────────────────

/// The four colour bands the Checks panel maps onto. `StatusDot` consumes
/// the matching `DotStatus`; the band keeps the semantics explicit + testable
/// independent of the dot component.
export type CheckBand = "green" | "red" | "amber" | "grey";

/// Map a `CheckRun` to its colour band (`design/15 §3.4`):
/// success→green; failure/timed_out/cancelled→red; in_progress/queued→amber;
/// neutral/skipped/stale/action_required/(unknown)→grey. A run still
/// `in_progress`/`queued` is amber regardless of a stale `conclusion`.
export function checkBand(run: CheckRun): CheckBand {
  if (run.status === "in_progress" || run.status === "queued") return "amber";
  switch (run.conclusion) {
    case "success":
      return "green";
    case "failure":
    case "timed_out":
    case "cancelled":
      return "red";
    case "neutral":
    case "skipped":
    case "stale":
    case "action_required":
      return "grey";
    default:
      return "grey";
  }
}

/// Aggregate band across a set of runs: red if ANY red, else amber if any
/// amber, else green if any green, else grey. Drives both the per-repo dot
/// and the workarea-wide disable-on-red (`design/15 §3.4`).
export function aggregateBand(runs: CheckRun[]): CheckBand {
  let band: CheckBand = "grey";
  for (const r of runs) {
    const b = checkBand(r);
    if (b === "red") return "red";
    if (b === "amber") band = "amber";
    else if (b === "green" && band !== "amber") band = "green";
  }
  return band;
}

/// True when the run set contains a red check — the workarea-wide
/// "Merge PR set" disable predicate, aggregated across the set.
export function hasRed(runs: CheckRun[]): boolean {
  return runs.some((r) => checkBand(r) === "red");
}

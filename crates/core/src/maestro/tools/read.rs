//! The 11 Maestro **read** tools (Task 405, `design/08 §5.1`), filling the
//! impls behind Task 401's FROZEN MCP schemas (`tools/mod.rs`).
//!
//! Each handler is a thin async fn that wraps an **existing** read API and maps
//! its result onto the exact frozen output JSON Task 401 registered — it never
//! re-shapes a tool's schema. The 11 tools are all
//! [`super::ToolKind::ReadOnly`] ⇒ they auto-approve under strict mode (Task 402
//! adds the `ToolClass::ReadOnly` bucket).
//!
//! ## Data sources (reuse, never reinvent — `design/08 §8`, PHASE4_PLANNING §2)
//!
//! | tool | source |
//! |---|---|
//! | `list_workspaces` | `concerto_persist::workspaces::list_all` + per-ws counts |
//! | `list_workareas` | `workareas::list_by_workspace` (all ws when absent) |
//! | `list_sessions` | `sessions::list_by_workarea` (all wa when absent) |
//! | `get_workspace_summary` | `workspaces::get` + active-workarea count + repo names |
//! | `get_workarea_summary` | Task 404's [`SummaryCache::get`] (verbatim) |
//! | `list_recent_activity` | workarea/session `last_activity_at` deltas since `since` |
//! | `list_active_schedules` | `schedules::list_active` |
//! | `read_inbox_summary` | typed empty stub (notifications = P5/Task 507) |
//! | `read_pr_set_for_workarea` | `pull_requests::list_by_workarea` (`(merge_order, pr_number)`) |
//! | `get_workarea_recent_commits` | `gix_wrap::recent_commits` over the workarea's worktree(s) |
//! | `cross_workarea_search` | `gix_wrap::grep` over every active workarea's worktree (live grep, R-6) |
//!
//! ## What stays out of this file (Scope — out)
//!
//! The 5 write tools (Task 406, `tools/write.rs`), the 2 side-channels (Task
//! 407, `tools/side.rs`), the `WorkareaSummary` cache build/refresh (Task 404,
//! consumed here as frozen), the privacy blanking over `get_workarea_summary`
//! (Task 413), the live inbox behind `read_inbox_summary` (Task 507), and
//! Tantivy search (V2.0). 405 returns the unblanked cache entry + a typed empty
//! inbox stub + live grep.
//!
//! ## Untracked workareas (404's handoff)
//!
//! [`SummaryCache::get`] returns `None` for a workarea the cache has not yet
//! tracked (404 handoff). `get_workarea_summary` surfaces that as a typed
//! `invalid_params` MCP error naming the workarea — never a fake-success / empty
//! `WorkareaSummary` (the 305/401 seam discipline). The live spine (402/414)
//! seeds the cache before the Maestro reads it.

use concerto_persist::{
    pull_requests, repositories, schedules, sessions, workareas, workspaces, Persistence,
    PullRequest, RepositoryId, Schedule, Session, Workarea, WorkareaId, Workspace, WorkspaceId,
};
use rmcp::ErrorData as McpError;
use serde_json::{json, Map, Value};

use crate::maestro::summary::{SummaryCache, WorkareaSummary};

/// Hard cap on `cross_workarea_search` hits (the `design/08 §8` tool guardrail:
/// never let an unbounded result set reach the LLM). The result reports
/// `truncated: true` once this many hits are collected.
pub const CROSS_SEARCH_HIT_CAP: usize = 100;

/// Default commit-walk depth for `get_workarea_recent_commits` (`design/08 §8`
/// guardrail — a bounded, recent slice, never the full history).
pub const RECENT_COMMITS_LIMIT: usize = 20;

/// Build the typed "not found" MCP error for a missing/untracked entity. Mirrors
/// 401's typed-error discipline (`invalid_params`, never empty-success).
fn not_found(what: &str, id: &str) -> McpError {
    McpError::invalid_params(format!("{what} not found: {id}"), None)
}

/// Map a `concerto_error::Error` from a wrapped read API onto an MCP internal
/// error (a read tool failing on I/O is an internal error, not bad input).
fn internal(err: impl std::fmt::Display) -> McpError {
    McpError::internal_error(format!("maestro read tool failed: {err}"), None)
}

/// Extract a required string argument from the validated tool-call args.
fn req_str(args: &Option<Map<String, Value>>, key: &str) -> Result<String, McpError> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| McpError::invalid_params(format!("missing required arg: {key}"), None))
}

/// Extract an optional string argument (absent / null → `None`).
fn opt_str(args: &Option<Map<String, Value>>, key: &str) -> Option<String> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract a required integer (`i64`) argument.
fn req_i64(args: &Option<Map<String, Value>>, key: &str) -> Result<i64, McpError> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| McpError::invalid_params(format!("missing required int arg: {key}"), None))
}

// ===========================================================================
// Hierarchy tools.
// ===========================================================================

/// `list_workspaces() → { workspaces: [{id, name, archived, n_workareas, n_repos}] }`.
///
/// Wraps `workspaces::list_all`; for each workspace, counts its non-archived
/// workareas (`workareas::list_by_workspace`) and attached repos
/// (`workspaces::list_repos`).
pub async fn list_workspaces(persist: &Persistence) -> Result<Value, McpError> {
    let pool = persist.readers();
    let workspaces = workspaces::list_all(pool).await.map_err(internal)?;
    let mut out = Vec::with_capacity(workspaces.len());
    for ws in &workspaces {
        let n_workareas = workareas::list_by_workspace(pool, &ws.id, false)
            .await
            .map_err(internal)?
            .len();
        let n_repos = workspaces::list_repos(pool, &ws.id)
            .await
            .map_err(internal)?
            .len();
        out.push(workspace_json(ws, n_workareas, n_repos));
    }
    Ok(json!({ "workspaces": out }))
}

fn workspace_json(ws: &Workspace, n_workareas: usize, n_repos: usize) -> Value {
    json!({
        "id": ws.id.0,
        "name": ws.name,
        "archived": ws.archived_at.is_some(),
        "n_workareas": n_workareas,
        "n_repos": n_repos,
    })
}

/// `list_workareas(workspace_id?) → { workareas: [{id, workspace_id, composer, branch, status, last_activity}] }`.
///
/// `workspace_id` absent ⇒ every workspace's non-archived workareas, concatenated.
pub async fn list_workareas(
    persist: &Persistence,
    workspace_id: Option<String>,
) -> Result<Value, McpError> {
    let pool = persist.readers();
    let mut areas: Vec<Workarea> = Vec::new();
    match workspace_id {
        Some(ws) => {
            areas = workareas::list_by_workspace(pool, &WorkspaceId(ws), false)
                .await
                .map_err(internal)?;
        }
        None => {
            for ws in workspaces::list_all(pool).await.map_err(internal)? {
                let mut a = workareas::list_by_workspace(pool, &ws.id, false)
                    .await
                    .map_err(internal)?;
                areas.append(&mut a);
            }
        }
    }
    let out: Vec<Value> = areas.iter().map(workarea_json).collect();
    Ok(json!({ "workareas": out }))
}

fn workarea_json(wa: &Workarea) -> Value {
    json!({
        "id": wa.id.0,
        "workspace_id": wa.workspace_id.0,
        "composer": wa.composer_name,
        "branch": wa.branch_name,
        "status": wa.status,
        "last_activity": wa.last_activity_at.unwrap_or(0),
    })
}

/// `list_sessions(workarea_id?) → { sessions: [{id, workarea_id, agent_kind, status, last_activity}] }`.
///
/// `workarea_id` absent ⇒ sessions across every non-archived workarea.
pub async fn list_sessions(
    persist: &Persistence,
    workarea_id: Option<String>,
) -> Result<Value, McpError> {
    let pool = persist.readers();
    let mut all: Vec<Session> = Vec::new();
    match workarea_id {
        Some(wa) => {
            all = sessions::list_by_workarea(pool, &WorkareaId(wa))
                .await
                .map_err(internal)?;
        }
        None => {
            for ws in workspaces::list_all(pool).await.map_err(internal)? {
                for wa in workareas::list_by_workspace(pool, &ws.id, false)
                    .await
                    .map_err(internal)?
                {
                    let mut s = sessions::list_by_workarea(pool, &wa.id)
                        .await
                        .map_err(internal)?;
                    all.append(&mut s);
                }
            }
        }
    }
    let out: Vec<Value> = all.iter().map(session_json).collect();
    Ok(json!({ "sessions": out }))
}

fn session_json(s: &Session) -> Value {
    // `last_activity` for a session is its most recent heartbeat, falling back
    // to its start time (a session always has `started_at`).
    let last_activity = s.last_heartbeat.unwrap_or(s.started_at);
    json!({
        "id": s.id.0,
        "workarea_id": s.workarea_id.0,
        "agent_kind": s.agent_kind,
        "status": s.status,
        "last_activity": last_activity,
    })
}

/// `get_workspace_summary(workspace_id) → { workspace, n_active_workareas, repos: [...] }`.
///
/// A deterministic, no-LLM rollup: the workspace name, its count of
/// non-archived workareas, and the names of its attached repos.
pub async fn get_workspace_summary(
    persist: &Persistence,
    workspace_id: String,
) -> Result<Value, McpError> {
    let pool = persist.readers();
    let ws_id = WorkspaceId(workspace_id);
    let ws = workspaces::get(pool, &ws_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("workspace", &ws_id.0))?;

    let n_active_workareas = workareas::list_by_workspace(pool, &ws_id, false)
        .await
        .map_err(internal)?
        .len();

    let repo_ids = workspaces::list_repos(pool, &ws_id)
        .await
        .map_err(internal)?;
    let mut repos = Vec::with_capacity(repo_ids.len());
    for rid in &repo_ids {
        // A workspace_repos row can outlive its repository only transiently; use
        // the id as a stable fallback name so the rollup never fails on a dangling row.
        let name = repositories::get(pool, rid)
            .await
            .map_err(internal)?
            .map(|r| r.name)
            .unwrap_or_else(|| rid.0.clone());
        repos.push(json!({ "id": rid.0, "name": name }));
    }

    Ok(json!({
        "workspace": ws.name,
        "n_active_workareas": n_active_workareas,
        "repos": repos,
    }))
}

/// `get_workarea_summary(workarea_id) → WorkareaSummary` — Task 404's cached
/// shape, returned **verbatim** (consumes PHASE4_PLANNING §4.4; never re-derived).
///
/// Returns a typed `invalid_params` error for an **untracked** workarea (404's
/// `get` → `None`); the live spine seeds the cache before the Maestro reads it.
/// Privacy blanking is layered on top by Task 413 — 405 returns the unblanked entry.
pub fn get_workarea_summary(cache: &SummaryCache, workarea_id: String) -> Result<Value, McpError> {
    let wa = WorkareaId(workarea_id);
    let summary = cache
        .get(&wa)
        .ok_or_else(|| not_found("workarea summary (untracked workarea)", &wa.0))?;
    Ok(workarea_summary_json(&summary))
}

/// Map Task 404's [`WorkareaSummary`] onto JSON. This is the one tool whose
/// frozen output schema 401 left as a minimal placeholder (`{ workarea_id }`)
/// pending 404; the full shape below is the `WorkareaSummary` field set
/// (i64-ms timestamps, per-repo hard facts) — a superset of 401's placeholder,
/// so it still validates against it.
fn workarea_summary_json(s: &WorkareaSummary) -> Value {
    let repos: Vec<Value> = s
        .repos
        .iter()
        .map(|r| {
            json!({
                "repository_id": r.repository_id.0,
                "repo_name": r.repo_name,
                "commits_ahead": r.commits_ahead,
                "files_changed": r.files_changed,
                "lines_added": r.lines_added,
                "lines_removed": r.lines_removed,
                "pr_state": r.pr_state,
                "ci_state": r.ci_state,
            })
        })
        .collect();
    let sessions: Vec<Value> = s
        .sessions
        .iter()
        .map(|sess| {
            json!({
                "session_id": sess.session_id.0,
                "agent_kind": format!("{:?}", sess.agent_kind),
                "model": sess.model,
                "status": sess.status,
                "last_turn_summary": sess.last_turn_summary,
            })
        })
        .collect();
    json!({
        "workarea_id": s.workarea_id.0,
        "workspace_id": s.workspace_id.0,
        "workspace_name": s.workspace_name,
        "composer_name": s.composer_name,
        "branch_name": s.branch_name,
        "status": s.status,
        "last_activity_at": s.last_activity_at,
        "sessions": sessions,
        "last_turn_summary": s.last_turn_summary,
        "last_3_turn_summaries": s.last_3_turn_summaries,
        "repos": repos,
        "blocked_on": s.blocked_on,
        "generated_at": s.generated_at,
        "generation": s.generation,
    })
}

// ===========================================================================
// Adjacent-state tools.
// ===========================================================================

/// `list_recent_activity(since) → { events: [Event] }`.
///
/// The bounded, newest-first activity feed since the given unix-ms `since`.
/// V1.0 sources hard activity facts from the existing
/// `workareas.last_activity_at` and `sessions` timestamps the summary cache
/// already consumes (no new event store): every workarea/session whose activity
/// crosses `since` is one event. Richer per-message events arrive when the
/// event/persist history readers are generalized (Task 409 consumes this same
/// feed).
pub async fn list_recent_activity(persist: &Persistence, since: i64) -> Result<Value, McpError> {
    let pool = persist.readers();
    let mut events: Vec<(i64, Value)> = Vec::new();

    for ws in workspaces::list_all(pool).await.map_err(internal)? {
        for wa in workareas::list_by_workspace(pool, &ws.id, false)
            .await
            .map_err(internal)?
        {
            if let Some(at) = wa.last_activity_at {
                if at >= since {
                    events.push((
                        at,
                        json!({
                            "kind": "workarea_activity",
                            "workarea_id": wa.id.0,
                            "workspace_id": wa.workspace_id.0,
                            "status": wa.status,
                            "at": at,
                        }),
                    ));
                }
            }
            for s in sessions::list_by_workarea(pool, &wa.id)
                .await
                .map_err(internal)?
            {
                let at = s.last_heartbeat.unwrap_or(s.started_at);
                if at >= since {
                    events.push((
                        at,
                        json!({
                            "kind": "session_activity",
                            "session_id": s.id.0,
                            "workarea_id": s.workarea_id.0,
                            "agent_kind": s.agent_kind,
                            "status": s.status,
                            "at": at,
                        }),
                    ));
                }
            }
        }
    }

    // Newest first.
    events.sort_by_key(|e| std::cmp::Reverse(e.0));
    let out: Vec<Value> = events.into_iter().map(|(_, v)| v).collect();
    Ok(json!({ "events": out }))
}

/// `list_active_schedules() → { schedules: [Schedule] }`.
///
/// Wraps `schedules::list_active(pool, now_ms)` (`paused = 0 AND expires_at >
/// now`). This avoids the per-workarea `SchedulerHandle::list_schedules` fan-out
/// (the handle has no global lister) by reading the existing `list_active`
/// persist reader directly — zero new persist surface (Implementation-notes
/// option (a), preferred).
pub async fn list_active_schedules(persist: &Persistence, now_ms: i64) -> Result<Value, McpError> {
    let pool = persist.readers();
    let schedules = schedules::list_active(pool, now_ms)
        .await
        .map_err(internal)?;
    let out: Vec<Value> = schedules.iter().map(schedule_json).collect();
    Ok(json!({ "schedules": out }))
}

fn schedule_json(s: &Schedule) -> Value {
    json!({
        "id": s.id.0,
        "workarea_id": s.workarea_id.0,
        "kind": s.kind,
        "interval_seconds": s.interval_seconds,
        "expires_at": s.expires_at,
        "last_run_at": s.last_run_at,
        "paused": s.paused,
        "prompt": s.prompt,
        "agent_kind": s.agent_kind,
    })
}

/// `read_inbox_summary() → InboxSummary { unread, items }` — the typed **empty
/// stub** kept for the handle-less sync dispatch path (`tools::dispatch`) + the
/// registration tests. The LIVE path is [`read_inbox_summary_live`] (Task 507).
pub fn read_inbox_summary() -> Value {
    json!({ "unread": 0, "items": [] })
}

/// `read_inbox_summary` — LIVE (Task 507): the up-to-20 most-recent UNREAD
/// notifications across all workspaces, newest-first, as the frozen
/// `InboxSummary { unread, items }` shape. Wired into [`dispatch_read`] (which
/// has the `Persistence` handle); the Maestro digest/chat consumes it.
pub async fn read_inbox_summary_live(persist: &Persistence) -> Result<Value, McpError> {
    const SUMMARY_LIMIT: u32 = 20;
    let rows = concerto_persist::notifications::list_inbox(
        persist.readers(),
        None,
        None,
        true,
        SUMMARY_LIMIT,
    )
    .await
    .map_err(|e| McpError::internal_error(format!("read_inbox_summary: {e}"), None))?;
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "kind": r.kind,
                "title": r.title,
                "severity": r.severity,
                "created_at_ms": r.created_at,
                "workarea_id": r.workarea_id,
            })
        })
        .collect();
    Ok(json!({ "unread": rows.len(), "items": items }))
}

/// `read_pr_set_for_workarea(workarea_id) → PrSetStatus`.
///
/// Wraps `pull_requests::list_by_workarea` (already ordered `(merge_order,
/// pr_number)`), mapping each row to the PR-set status shape.
pub async fn read_pr_set_for_workarea(
    persist: &Persistence,
    workarea_id: String,
) -> Result<Value, McpError> {
    let pool = persist.readers();
    let wa = WorkareaId(workarea_id);
    let prs = pull_requests::list_by_workarea(pool, &wa)
        .await
        .map_err(internal)?;
    let out: Vec<Value> = prs.iter().map(pr_json).collect();
    Ok(json!({
        "workarea_id": wa.0,
        "pull_requests": out,
        "n_prs": prs.len(),
    }))
}

fn pr_json(pr: &PullRequest) -> Value {
    json!({
        "id": pr.id.0,
        "repository_id": pr.repository_id.0,
        "pr_number": pr.pr_number,
        "state": pr.state,
        "title": pr.title,
        "url": pr.url,
        "merge_order": pr.merge_order,
    })
}

/// `get_workarea_recent_commits(workarea_id, repo_id?) → { commits: [Commit] }`.
///
/// Resolves the workarea's worktree dir(s) (`workareas::list_workarea_repos`),
/// walks each via `gix_wrap::recent_commits(worktree, branch, RECENT_COMMITS_LIMIT)`,
/// and tags each commit with its `repository_id`. `repo_id` absent ⇒ every repo
/// in the workarea; present ⇒ only that repo. Newest-first per repo.
pub async fn get_workarea_recent_commits(
    persist: &Persistence,
    workarea_id: String,
    repo_id: Option<String>,
) -> Result<Value, McpError> {
    let pool = persist.readers();
    let wa = WorkareaId(workarea_id);
    let wa_row = workareas::get(pool, &wa)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("workarea", &wa.0))?;
    let branch = wa_row.branch_name;

    let mut repo_worktrees = workareas::list_workarea_repos(pool, &wa)
        .await
        .map_err(internal)?;
    if let Some(rid) = &repo_id {
        repo_worktrees.retain(|(r, _)| &r.0 == rid);
    }

    let mut commits = Vec::new();
    for (rid, worktree) in &repo_worktrees {
        let walked = concerto_gix_wrap::recent_commits(
            std::path::Path::new(worktree),
            &branch,
            RECENT_COMMITS_LIMIT,
        )
        .await
        .map_err(internal)?;
        for c in walked {
            commits.push(commit_json(rid, &c));
        }
    }

    Ok(json!({ "commits": commits }))
}

fn commit_json(repo: &RepositoryId, c: &concerto_gix_wrap::Commit) -> Value {
    json!({
        "repository_id": repo.0,
        "oid": c.oid,
        "short_oid": c.short_oid,
        "summary": c.summary,
        "author": c.author,
        "committed_at": c.committed_at,
    })
}

/// `cross_workarea_search(query) → { hits: [Hit], truncated }` — V1.0 **live
/// grep** over every active (non-archived) workarea's worktree(s) (`design/08
/// R-6`; Tantivy is V2.0).
///
/// For each active workarea → each repo worktree → `gix_wrap::grep` (a
/// `git grep --fixed-strings` shell-out, cross-platform). Hits map to the `Hit`
/// shape (`{workarea, repo, path, line, snippet}`); the total is capped to
/// [`CROSS_SEARCH_HIT_CAP`] (the `design/08 §8` guardrail) and the result
/// reports `truncated: true` once the cap is reached.
pub async fn cross_workarea_search(
    persist: &Persistence,
    query: String,
) -> Result<Value, McpError> {
    let pool = persist.readers();
    let mut hits: Vec<Value> = Vec::new();
    let mut truncated = false;

    'outer: for ws in workspaces::list_all(pool).await.map_err(internal)? {
        for wa in workareas::list_by_workspace(pool, &ws.id, false)
            .await
            .map_err(internal)?
        {
            for (rid, worktree) in workareas::list_workarea_repos(pool, &wa.id)
                .await
                .map_err(internal)?
            {
                let remaining = CROSS_SEARCH_HIT_CAP.saturating_sub(hits.len());
                if remaining == 0 {
                    truncated = true;
                    break 'outer;
                }
                // Ask for one past the remaining budget so we can detect that
                // this worktree alone overflowed the cap.
                let found =
                    concerto_gix_wrap::grep(std::path::Path::new(&worktree), &query, remaining + 1)
                        .await
                        .map_err(internal)?;
                for hit in found.into_iter().take(remaining) {
                    hits.push(json!({
                        "workarea": wa.id.0,
                        "repo": rid.0,
                        "path": hit.path,
                        "line": hit.line,
                        "snippet": hit.snippet,
                    }));
                }
                if hits.len() >= CROSS_SEARCH_HIT_CAP {
                    truncated = true;
                    break 'outer;
                }
            }
        }
    }

    Ok(json!({ "hits": hits, "truncated": truncated }))
}

// ===========================================================================
// Argument-deserializing entry points (the frozen 401 arg sets).
// ===========================================================================

/// Dispatch a read tool by its frozen name, deserializing `args` per 401's
/// frozen input schema and returning the frozen output JSON.
///
/// This is the seam the live MCP server (`super::super::mcp`, once 402/414 wire
/// Core handles into `MaestroMcpServer`) calls in place of 401's
/// typed-unimplemented arm for the 11 read tools. `now_ms` is the caller's clock
/// (the supervisor's wall clock in prod; a fixed value in tests) used by
/// `list_active_schedules`.
pub async fn dispatch_read(
    name: &str,
    args: Option<Map<String, Value>>,
    persist: &Persistence,
    cache: &SummaryCache,
    now_ms: i64,
) -> Result<Value, McpError> {
    match name {
        "list_workspaces" => list_workspaces(persist).await,
        "list_workareas" => list_workareas(persist, opt_str(&args, "workspace_id")).await,
        "list_sessions" => list_sessions(persist, opt_str(&args, "workarea_id")).await,
        "get_workspace_summary" => {
            get_workspace_summary(persist, req_str(&args, "workspace_id")?).await
        }
        "get_workarea_summary" => get_workarea_summary(cache, req_str(&args, "workarea_id")?),
        "list_recent_activity" => list_recent_activity(persist, req_i64(&args, "since")?).await,
        "list_active_schedules" => list_active_schedules(persist, now_ms).await,
        "read_inbox_summary" => read_inbox_summary_live(persist).await,
        "read_pr_set_for_workarea" => {
            read_pr_set_for_workarea(persist, req_str(&args, "workarea_id")?).await
        }
        "get_workarea_recent_commits" => {
            get_workarea_recent_commits(
                persist,
                req_str(&args, "workarea_id")?,
                opt_str(&args, "repo_id"),
            )
            .await
        }
        "cross_workarea_search" => cross_workarea_search(persist, req_str(&args, "query")?).await,
        other => Err(McpError::invalid_params(
            format!("not a maestro read tool: {other}"),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_supervisor::actor::AgentKind;
    use crate::maestro::summary::{RepoSummary, SessionSummary};
    use concerto_persist::{
        NewChat, NewPullRequest, NewRepository, NewSchedule, NewSession, NewWorkarea,
        NewWorkareaRepo, NewWorkspace, PersistenceConfig, PullRequestId, ScheduleId, SessionId,
    };
    use std::path::Path;
    use tokio::process::Command;

    // ---- fixture builders -------------------------------------------------

    async fn fresh() -> (tempfile::TempDir, Persistence) {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open");
        (dir, persist)
    }

    async fn add_workspace(p: &Persistence, id: &str, name: &str) {
        let mut w = p.writer().await;
        workspaces::insert(
            &mut w,
            NewWorkspace {
                id: WorkspaceId(id.into()),
                name: name.into(),
                slug: id.into(),
                icon: None,
                description: None,
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .expect("insert ws");
    }

    async fn add_repo(p: &Persistence, id: &str, name: &str, local_path: &str) {
        let mut w = p.writer().await;
        repositories::insert(
            &mut w,
            NewRepository {
                id: RepositoryId(id.into()),
                name: name.into(),
                url: format!("file:///{id}"),
                local_path: local_path.into(),
                clone_strategy: "full".into(),
                default_branch: "main".into(),
            },
        )
        .await
        .expect("insert repo");
    }

    async fn attach_repo(p: &Persistence, ws: &str, repo: &str) {
        let mut w = p.writer().await;
        workspaces::update_repos(
            &mut w,
            &WorkspaceId(ws.into()),
            &[concerto_persist::WorkspaceRepoCones::empty_cones(
                RepositoryId(repo.into()),
            )],
        )
        .await
        .expect("attach repo");
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_workarea(
        p: &Persistence,
        id: &str,
        ws: &str,
        composer: &str,
        branch: &str,
        worktree_root: &str,
        status: &str,
        last_activity: Option<i64>,
    ) {
        let mut w = p.writer().await;
        workareas::insert(
            &mut w,
            NewWorkarea {
                id: WorkareaId(id.into()),
                workspace_id: ws.into(),
                composer_name: composer.into(),
                branch_name: branch.into(),
                worktree_root: worktree_root.into(),
                status: status.into(),
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .expect("insert workarea");
        if let Some(at) = last_activity {
            // No persist setter for `last_activity_at`; write it directly.
            sqlx::query("UPDATE workareas SET last_activity_at = ? WHERE id = ?")
                .bind(at)
                .bind(id)
                .execute(&mut *w)
                .await
                .expect("set last_activity");
        }
    }

    async fn attach_workarea_repo(p: &Persistence, wa: &str, repo: &str, worktree: &str) {
        let mut w = p.writer().await;
        workareas::insert_workarea_repo(
            &mut w,
            NewWorkareaRepo {
                workarea_id: WorkareaId(wa.into()),
                repository_id: RepositoryId(repo.into()),
                worktree_path: worktree.into(),
                branch_override: None,
                sparse_cones_json: NewWorkareaRepo::empty_cones(),
            },
        )
        .await
        .expect("insert workarea_repo");
    }

    async fn add_session(
        p: &Persistence,
        id: &str,
        wa: &str,
        agent_kind: &str,
        status: &str,
        started_at: i64,
    ) {
        use sqlx::Connection;
        let mut w = p.writer().await;
        let chat_id = format!("chat-{id}");
        // `chats.session_id` ↔ `sessions.chat_id` is a circular FK, so the real
        // session-creation path (agent_supervisor/actor.rs) inserts both inside
        // one transaction with `PRAGMA defer_foreign_keys = ON`; mirror that.
        let mut tx = w.begin().await.expect("tx");
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .expect("defer fks");
        sessions::insert_chat(
            &mut tx,
            NewChat {
                id: chat_id.clone(),
                session_id: Some(id.into()),
                kind: "session".into(),
                created_at: started_at,
            },
        )
        .await
        .expect("insert chat");
        sessions::insert(
            &mut tx,
            NewSession {
                id: SessionId(id.into()),
                workarea_id: WorkareaId(wa.into()),
                chat_id,
                agent_kind: agent_kind.into(),
                agent_version: None,
                model: Some("claude-x".into()),
                mode: None,
                host_pid: None,
                host_socket: None,
                pty_cookie: None,
                external_session_id: None,
                permission_mode: "strict".into(),
                bypass_destructive_guard: false,
                started_at,
                status: status.into(),
                last_acked_seq: 0,
            },
        )
        .await
        .expect("insert session");
        tx.commit().await.expect("commit");
    }

    #[allow(clippy::too_many_arguments)]
    async fn add_pr(
        p: &Persistence,
        id: &str,
        wa: &str,
        repo: &str,
        pr_number: i64,
        state: &str,
        merge_order: i64,
    ) {
        let mut w = p.writer().await;
        pull_requests::upsert(
            &mut w,
            NewPullRequest {
                id: PullRequestId(id.into()),
                workarea_id: WorkareaId(wa.into()),
                repository_id: RepositoryId(repo.into()),
                provider: "github".into(),
                pr_number,
                base_ref: "main".into(),
                head_ref: "feature".into(),
                state: state.into(),
                title: format!("PR {pr_number}"),
                body: String::new(),
                url: format!("https://example/pr/{pr_number}"),
                head_sha: "deadbeef".into(),
                merge_order,
                external_id: String::new(),
                repository_full_name: "acme/repo".into(),
                created_at: 1,
                updated_at: 1,
            },
        )
        .await
        .expect("upsert pr");
    }

    async fn add_schedule(p: &Persistence, id: &str, wa: &str, expires_at: i64) {
        let mut w = p.writer().await;
        schedules::insert(
            &mut w,
            NewSchedule {
                id: ScheduleId(id.into()),
                workarea_id: WorkareaId(wa.into()),
                kind: "loop".into(),
                interval_seconds: 60,
                expires_at,
                last_run_at: None,
                paused: false,
                prompt: "tick".into(),
                agent_kind: "claude".into(),
                created_at: 1,
            },
        )
        .await
        .expect("insert schedule");
    }

    async fn git(args: &[&str], cwd: &Path) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "Ada")
            .env("GIT_AUTHOR_EMAIL", "ada@example.com")
            .env("GIT_COMMITTER_NAME", "Ada")
            .env("GIT_COMMITTER_EMAIL", "ada@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .await
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Build a worktree on `branch` with `files` (name → content) committed,
    /// returning its path inside `dir`.
    async fn make_worktree(dir: &Path, branch: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let wt = dir.to_path_buf();
        git(&["init", "-b", branch, "."], &wt).await;
        for (name, content) in files {
            tokio::fs::write(wt.join(name), content).await.unwrap();
        }
        git(&["add", "."], &wt).await;
        git(&["commit", "-m", "seed commit"], &wt).await;
        wt
    }

    /// A two-workspace / three-workarea seeded fixture (the §"Scope — in"
    /// shape): ws-1 with wa-a (2 sessions) + wa-b, ws-2 with wa-c; 1 repo
    /// attached to ws-1; a PR row + a schedule row on wa-a.
    async fn seeded() -> (tempfile::TempDir, Persistence) {
        let (dir, p) = fresh().await;
        add_workspace(&p, "ws-1", "Alpha").await;
        add_workspace(&p, "ws-2", "Beta").await;
        add_repo(&p, "repo-1", "core", "/tmp/repo-1").await;
        attach_repo(&p, "ws-1", "repo-1").await;
        add_workarea(
            &p,
            "wa-a",
            "ws-1",
            "bach",
            "concerto/bach",
            "/tmp/wa-a",
            "running",
            Some(1000),
        )
        .await;
        add_workarea(
            &p,
            "wa-b",
            "ws-1",
            "liszt",
            "concerto/liszt",
            "/tmp/wa-b",
            "paused",
            Some(500),
        )
        .await;
        add_workarea(
            &p,
            "wa-c",
            "ws-2",
            "haydn",
            "concerto/haydn",
            "/tmp/wa-c",
            "active",
            None,
        )
        .await;
        add_session(&p, "sess-1", "wa-a", "claude", "running", 900).await;
        add_session(&p, "sess-2", "wa-a", "codex", "finished", 800).await;
        // Two PRs on wa-a — one per repo (the upsert conflict key is
        // `(workarea_id, repository_id)`, so each repo holds one PR row). repo-2
        // exists for the PR FK but is intentionally NOT attached to ws-1, so the
        // workspace repo count (which counts `workspace_repos`) stays 1.
        add_repo(&p, "repo-2", "docs", "/tmp/repo-2").await;
        add_pr(&p, "pr-1", "wa-a", "repo-1", 7, "open", 1).await;
        add_pr(&p, "pr-2", "wa-a", "repo-2", 3, "merged", 0).await;
        add_schedule(&p, "sched-1", "wa-a", 1_000_000).await;
        (dir, p)
    }

    // ---- hierarchy tools --------------------------------------------------

    #[tokio::test]
    async fn list_workspaces_counts_workareas_and_repos() {
        let (_d, p) = seeded().await;
        let v = list_workspaces(&p).await.expect("ok");
        let arr = v["workspaces"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // Sorted by name: Alpha (ws-1) first.
        let alpha = &arr[0];
        assert_eq!(alpha["id"], "ws-1");
        assert_eq!(alpha["name"], "Alpha");
        assert_eq!(alpha["archived"], false);
        assert_eq!(alpha["n_workareas"], 2); // wa-a + wa-b
        assert_eq!(alpha["n_repos"], 1);
        let beta = &arr[1];
        assert_eq!(beta["id"], "ws-2");
        assert_eq!(beta["n_workareas"], 1);
        assert_eq!(beta["n_repos"], 0);
    }

    #[tokio::test]
    async fn list_workareas_all_and_filtered() {
        let (_d, p) = seeded().await;
        // All workspaces.
        let all = list_workareas(&p, None).await.expect("ok");
        assert_eq!(all["workareas"].as_array().unwrap().len(), 3);
        // Filtered to ws-1.
        let ws1 = list_workareas(&p, Some("ws-1".into())).await.expect("ok");
        let arr = ws1["workareas"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let a = arr.iter().find(|x| x["id"] == "wa-a").unwrap();
        assert_eq!(a["workspace_id"], "ws-1");
        assert_eq!(a["composer"], "bach");
        assert_eq!(a["branch"], "concerto/bach");
        assert_eq!(a["status"], "running");
        assert_eq!(a["last_activity"], 1000);
    }

    #[tokio::test]
    async fn list_sessions_all_and_filtered() {
        let (_d, p) = seeded().await;
        let all = list_sessions(&p, None).await.expect("ok");
        assert_eq!(all["sessions"].as_array().unwrap().len(), 2);
        let wa_a = list_sessions(&p, Some("wa-a".into())).await.expect("ok");
        let arr = wa_a["sessions"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let s = arr.iter().find(|x| x["id"] == "sess-1").unwrap();
        assert_eq!(s["workarea_id"], "wa-a");
        assert_eq!(s["agent_kind"], "claude");
        assert_eq!(s["status"], "running");
        assert_eq!(s["last_activity"], 900); // started_at (no heartbeat)
    }

    #[tokio::test]
    async fn get_workspace_summary_rolls_up() {
        let (_d, p) = seeded().await;
        let v = get_workspace_summary(&p, "ws-1".into()).await.expect("ok");
        assert_eq!(v["workspace"], "Alpha");
        assert_eq!(v["n_active_workareas"], 2);
        let repos = v["repos"].as_array().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0]["id"], "repo-1");
        assert_eq!(repos[0]["name"], "core");
    }

    #[tokio::test]
    async fn get_workspace_summary_unknown_is_typed_error() {
        let (_d, p) = seeded().await;
        let err = get_workspace_summary(&p, "nope".into())
            .await
            .expect_err("missing ws");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn get_workarea_summary_returns_cache_entry_verbatim() {
        let mut cache = SummaryCache::with_system_clock();
        let summary = WorkareaSummary {
            workarea_id: WorkareaId("wa-a".into()),
            workspace_id: WorkspaceId("ws-1".into()),
            workspace_name: "Alpha".into(),
            composer_name: "bach".into(),
            branch_name: "concerto/bach".into(),
            status: "running".into(),
            last_activity_at: 1000,
            sessions: vec![SessionSummary {
                session_id: SessionId("sess-1".into()),
                agent_kind: AgentKind::Claude,
                model: "claude-x".into(),
                status: "running".into(),
                last_turn_summary: "did a thing".into(),
            }],
            last_turn_summary: "did a thing".into(),
            last_3_turn_summaries: vec!["did a thing".into()],
            repos: vec![RepoSummary {
                repository_id: RepositoryId("repo-1".into()),
                repo_name: "core".into(),
                commits_ahead: 3,
                files_changed: 2,
                lines_added: 10,
                lines_removed: 4,
                pr_state: Some("open".into()),
                ci_state: Some("success".into()),
            }],
            blocked_on: None,
            generated_at: 0,
            generation: 0,
        };
        cache.upsert(summary);

        let v = get_workarea_summary(&cache, "wa-a".into()).expect("tracked");
        assert_eq!(v["workarea_id"], "wa-a");
        assert_eq!(v["workspace_name"], "Alpha");
        assert_eq!(v["branch_name"], "concerto/bach");
        assert_eq!(v["last_turn_summary"], "did a thing");
        let repos = v["repos"].as_array().unwrap();
        assert_eq!(repos[0]["commits_ahead"], 3);
        assert_eq!(repos[0]["pr_state"], "open");
        assert_eq!(repos[0]["ci_state"], "success");
        assert_eq!(v["sessions"][0]["session_id"], "sess-1");
    }

    #[test]
    fn get_workarea_summary_untracked_is_typed_error() {
        let cache = SummaryCache::with_system_clock();
        let err = get_workarea_summary(&cache, "ghost".into()).expect_err("untracked");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("untracked"));
    }

    // ---- adjacent-state tools ---------------------------------------------

    #[tokio::test]
    async fn list_recent_activity_filters_and_sorts_desc() {
        let (_d, p) = seeded().await;
        // since=600: wa-a(1000), sess-1(900), sess-2(800), wa-b(500 excluded).
        let v = list_recent_activity(&p, 600).await.expect("ok");
        let events = v["events"].as_array().unwrap();
        // wa-a + sess-1 + sess-2 = 3 (wa-b at 500 excluded, wa-c has no activity).
        assert_eq!(events.len(), 3);
        // Newest first.
        let times: Vec<i64> = events.iter().map(|e| e["at"].as_i64().unwrap()).collect();
        assert_eq!(times, vec![1000, 900, 800]);
        assert_eq!(events[0]["kind"], "workarea_activity");
        assert_eq!(events[0]["workarea_id"], "wa-a");
    }

    #[tokio::test]
    async fn list_active_schedules_non_empty() {
        let (_d, p) = seeded().await;
        // now well before expiry (1_000_000).
        let v = list_active_schedules(&p, 1).await.expect("ok");
        let arr = v["schedules"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "sched-1");
        assert_eq!(arr[0]["workarea_id"], "wa-a");
        assert_eq!(arr[0]["paused"], false);
        // After expiry → empty.
        let none = list_active_schedules(&p, 2_000_000).await.expect("ok");
        assert!(none["schedules"].as_array().unwrap().is_empty());
    }

    #[test]
    fn read_inbox_summary_is_typed_empty_stub() {
        let v = read_inbox_summary();
        assert_eq!(v["unread"], 0);
        assert!(v["items"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn read_pr_set_orders_by_merge_order_then_pr_number() {
        let (_d, p) = seeded().await;
        let v = read_pr_set_for_workarea(&p, "wa-a".into())
            .await
            .expect("ok");
        assert_eq!(v["workarea_id"], "wa-a");
        assert_eq!(v["n_prs"], 2);
        let prs = v["pull_requests"].as_array().unwrap();
        // pr-2 has merge_order 0, pr-1 has merge_order 1 → pr-2 first.
        assert_eq!(prs[0]["id"], "pr-2");
        assert_eq!(prs[0]["state"], "merged");
        assert_eq!(prs[0]["merge_order"], 0);
        assert_eq!(prs[1]["id"], "pr-1");
        assert_eq!(prs[1]["pr_number"], 7);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_workarea_recent_commits_newest_first_on_fixture_repo() {
        let (dir, p) = fresh().await;
        add_workspace(&p, "ws-1", "Alpha").await;
        add_repo(&p, "repo-1", "core", "/tmp/repo-1").await;
        // The workarea branch is `main` so recent_commits walks it.
        add_workarea(
            &p,
            "wa-a",
            "ws-1",
            "bach",
            "main",
            "/tmp/wa-a",
            "running",
            Some(1),
        )
        .await;

        let wt_dir = dir.path().join("wt");
        tokio::fs::create_dir_all(&wt_dir).await.unwrap();
        let wt = make_worktree(&wt_dir, "main", &[("a.txt", "one\n")]).await;
        // Two more commits so order is testable.
        tokio::fs::write(wt.join("b.txt"), "two\n").await.unwrap();
        git(&["add", "."], &wt).await;
        git(&["commit", "-m", "second"], &wt).await;
        tokio::fs::write(wt.join("c.txt"), "three\n").await.unwrap();
        git(&["add", "."], &wt).await;
        git(&["commit", "-m", "third"], &wt).await;

        attach_workarea_repo(&p, "wa-a", "repo-1", wt.to_str().unwrap()).await;

        let v = get_workarea_recent_commits(&p, "wa-a".into(), None)
            .await
            .expect("ok");
        let commits = v["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0]["summary"], "third");
        assert_eq!(commits[1]["summary"], "second");
        assert_eq!(commits[2]["summary"], "seed commit");
        assert_eq!(commits[0]["repository_id"], "repo-1");
    }

    // ---- cross_workarea_search (live grep) --------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn cross_workarea_search_finds_planted_hit_and_misses_absent() {
        let (dir, p) = fresh().await;
        add_workspace(&p, "ws-1", "Alpha").await;
        add_repo(&p, "repo-1", "core", "/tmp/repo-1").await;
        add_workarea(
            &p,
            "wa-a",
            "ws-1",
            "bach",
            "main",
            "/tmp/wa-a",
            "running",
            Some(1),
        )
        .await;

        let wt_dir = dir.path().join("wt");
        tokio::fs::create_dir_all(&wt_dir).await.unwrap();
        let wt = make_worktree(
            &wt_dir,
            "main",
            &[("auth.rs", "fn login() { let NEEDLE_TOKEN = 1; }\n")],
        )
        .await;
        attach_workarea_repo(&p, "wa-a", "repo-1", wt.to_str().unwrap()).await;

        let hit = cross_workarea_search(&p, "NEEDLE_TOKEN".into())
            .await
            .expect("ok");
        let hits = hit["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["workarea"], "wa-a");
        assert_eq!(hits[0]["repo"], "repo-1");
        assert_eq!(hits[0]["path"], "auth.rs");
        assert_eq!(hits[0]["line"], 1);
        assert!(hits[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("NEEDLE_TOKEN"));
        assert_eq!(hit["truncated"], false);

        // A non-matching query → empty.
        let none = cross_workarea_search(&p, "definitely-absent-zzz".into())
            .await
            .expect("ok");
        assert!(none["hits"].as_array().unwrap().is_empty());
        assert_eq!(none["truncated"], false);
    }

    // ---- frozen-schema round-trip + dispatch ------------------------------

    #[tokio::test]
    async fn outputs_validate_against_frozen_401_schemas() {
        let (_d, p) = seeded().await;
        let cache = SummaryCache::with_system_clock();
        let descriptors = crate::maestro::tools::all_tools();
        let schema_of = |name: &str| {
            descriptors
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.output_schema.clone())
                .unwrap()
        };

        // The shapes whose 401 output_schema declares `required` keys at the
        // top level — assert each required key is present (round-trip against
        // the frozen contract). `get_workarea_summary`'s 401 schema is the
        // minimal `{workarea_id}` placeholder; our superset still satisfies it.
        let check = |name: &str, out: &Value| {
            let schema = schema_of(name);
            if let Some(req) = schema.get("required").and_then(|r| r.as_array()) {
                for key in req {
                    let k = key.as_str().unwrap();
                    assert!(
                        out.get(k).is_some(),
                        "{name} output missing frozen-required key `{k}`: {out}"
                    );
                }
            }
            assert!(out.is_object(), "{name} output must be a JSON object");
        };

        check("list_workspaces", &list_workspaces(&p).await.unwrap());
        check("list_workareas", &list_workareas(&p, None).await.unwrap());
        check("list_sessions", &list_sessions(&p, None).await.unwrap());
        check(
            "get_workspace_summary",
            &get_workspace_summary(&p, "ws-1".into()).await.unwrap(),
        );
        check(
            "list_recent_activity",
            &list_recent_activity(&p, 0).await.unwrap(),
        );
        check(
            "list_active_schedules",
            &list_active_schedules(&p, 1).await.unwrap(),
        );
        check("read_inbox_summary", &read_inbox_summary());
        check(
            "read_pr_set_for_workarea",
            &read_pr_set_for_workarea(&p, "wa-a".into()).await.unwrap(),
        );
        let _ = &cache; // get_workarea_summary covered in its own test (needs a seeded cache)
    }

    #[tokio::test]
    async fn dispatch_read_routes_and_rejects_non_read_tool() {
        let (_d, p) = seeded().await;
        let cache = SummaryCache::with_system_clock();
        let v = dispatch_read("list_workspaces", None, &p, &cache, 1)
            .await
            .expect("ok");
        assert_eq!(v["workspaces"].as_array().unwrap().len(), 2);

        // A write-tool name is not a read tool → typed invalid_params.
        let err = dispatch_read("create_workspace", None, &p, &cache, 1)
            .await
            .expect_err("not a read tool");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);

        // A required-arg-missing read tool → typed invalid_params.
        let err2 = dispatch_read("get_workspace_summary", None, &p, &cache, 1)
            .await
            .expect_err("missing arg");
        assert_eq!(err2.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}

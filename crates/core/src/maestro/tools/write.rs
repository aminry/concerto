//! The 5 Maestro **write** tools (Task 406, `design/08 §5.1`), filling the
//! impls behind Task 401's FROZEN MCP schemas (`tools/mod.rs`).
//!
//! Each tool MUTATES Concerto — `route_prompt_to_session` / `fanout_to_sessions`
//! drive [`crate::agent_supervisor::AgentSupervisorHandle::send_input`];
//! `create_workspace` / `create_workarea` drive the Workspace/Workarea managers;
//! `set_workarea_paused` drives `transition_workarea(Pause|Resume)`. Because the
//! Maestro session always runs **strict** (`MAESTRO_PERMISSION_MODE`) and these
//! 5 tools classify non-[`crate::security::ToolClass::ReadOnly`] (they fall
//! through to `Restricted` in `security/tool_classes.rs`), the built
//! [`crate::security::PermissionResolver`] returns
//! [`crate::security::Decision::MustAsk`] for each — surfaced as the existing
//! `AwaitingApproval` / `ResolveApproval` **confirmation chip** (Task 33 / 402).
//!
//! ## The load-bearing rule — gate FIRST, mutate SECOND (§4.8 / D4, `design/08 R-2`)
//!
//! Every write tool calls [`MaestroGate::confirm`] BEFORE it touches a manager
//! or `send_input`. The order in each body is, strictly:
//!
//! 1. resolve + validate the frozen args,
//! 2. classify (Restricted) + `resolver.decide ⇒ MustAsk` under strict,
//! 3. emit `AwaitingApproval` + park on the user's decision,
//! 4. on approve → mutate; on deny → a typed `"user declined"` tool result and
//!    **no mutation**.
//!
//! There is **no bypass**. A reviewer can read each body and see the gate
//! dominate the mutation call. `fanout_to_sessions` gates **once** for the whole
//! fanout (the user confirms "send to N sessions" a single time, `design/08
//! §5.1`), then sends per target and collects per-session ok/err so a partial
//! failure surfaces instead of being swallowed.
//!
//! ## Reusable inner fns for Task 411
//!
//! [`do_create_workspace`] / [`do_create_workarea`] are the create-flow inner
//! fns Task 411 (`create_workspace_from_description`) wraps after issue-parse +
//! cone-suggest — they take an already-mapped spec and return the new id, so 411
//! composes them without re-implementing the create. They live in their own
//! region of this file (the "create flow" region §8.1) so 411's additions merge
//! cleanly.
//!
//! ## The seam discipline (305 / 401)
//!
//! A closed/missing session (`route`/`fanout`), a missing/archived workspace
//! (`create_workarea`), an illegal pause (`transition_workarea` ⇒
//! [`concerto_error::Error::Policy`]), or a user decline each maps to a **typed**
//! MCP error result — never `todo!()`/`unimplemented!()`, never `unwrap()`, never
//! empty-success.
//!
//! ## What stays out of this file (Scope — out)
//!
//! The Maestro LLM agent loop that *chooses* which tool to call (Task 402); the
//! `create_workspace_from_description` issue-parse / cone-suggest front-end (Task
//! 411, wraps the `do_create_*` fns here); the 2 side-channels (Task 407,
//! `tools/side.rs`); the 11 read tools (Task 405, `tools/read.rs`); and the
//! Desktop confirmation-chip render + user tap UX (Task 415 / Phase-4 Tier-3).
//! This file proves the gate fires and blocks the mutation, not the pixels.

use async_trait::async_trait;
use rmcp::ErrorData as McpError;
use serde_json::{json, Map, Value};

use crate::security::{Decision, PermissionResolver};
use concerto_persist::SessionId;

// ===========================================================================
// Argument helpers (the frozen 401 arg sets).
// ===========================================================================

/// Extract a required string argument from the validated tool-call args.
fn req_str(args: &Option<Map<String, Value>>, key: &str) -> Result<String, McpError> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| McpError::invalid_params(format!("missing required arg: {key}"), None))
}

/// Extract an optional string field from a spec/object value.
fn opt_str(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Extract a required bool argument.
fn req_bool(args: &Option<Map<String, Value>>, key: &str) -> Result<bool, McpError> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_bool())
        .ok_or_else(|| McpError::invalid_params(format!("missing required bool arg: {key}"), None))
}

/// Extract a required array-of-strings argument (the frozen `session_ids`).
fn req_str_array(args: &Option<Map<String, Value>>, key: &str) -> Result<Vec<String>, McpError> {
    let arr = args
        .as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            McpError::invalid_params(format!("missing required array arg: {key}"), None)
        })?;
    arr.iter()
        .map(|v| {
            v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                McpError::invalid_params(format!("{key} must be an array of strings"), None)
            })
        })
        .collect()
}

/// Extract a required object argument (the frozen `spec`).
fn req_obj<'a>(args: &'a Option<Map<String, Value>>, key: &str) -> Result<&'a Value, McpError> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .filter(|v| v.is_object())
        .ok_or_else(|| {
            McpError::invalid_params(format!("missing required object arg: {key}"), None)
        })
}

/// Map a `concerto_error::Error` from a wrapped mutation onto an MCP error.
///
/// A bad-input / precondition failure ([`Error::Validation`] / [`Error::Policy`]
/// / [`Error::NotFound`]) is `invalid_params` (the agent asked for something the
/// system rejected); an I/O / internal failure is `internal_error`. Either way
/// it is a **typed** error result, never a panic or empty-success.
fn map_err(err: concerto_error::Error) -> McpError {
    use concerto_error::Error;
    match err {
        Error::Validation(_) | Error::Policy(_) | Error::NotFound(_) => {
            McpError::invalid_params(format!("maestro write tool rejected: {err}"), None)
        }
        other => McpError::internal_error(format!("maestro write tool failed: {other}"), None),
    }
}

// ===========================================================================
// The strict confirmation gate (the §4.8 / D4 chip-gate, `design/08 R-2`).
// ===========================================================================

/// The confirmation a write tool requests before it mutates.
///
/// One value per gated tool call; the [`MaestroGate`] decides it under the
/// Maestro session's strict permission mode and (in production) by parking on
/// the user's `AwaitingApproval` → `ResolveApproval` chip.
#[derive(Debug, Clone)]
pub struct GateRequest {
    /// The frozen tool name (e.g. `"route_prompt_to_session"`).
    pub tool: &'static str,
    /// Human-readable one-line summary surfaced on the chip
    /// (`AgentEvent::AwaitingApproval.summary`).
    pub summary: String,
    /// `Some(label)` ⇒ render the chip red ("destructive"). All 5 write tools
    /// here are non-destructive (they create or send, they do not delete) ⇒
    /// `None` (implementation note in the task file).
    pub destructive_label: Option<String>,
    /// Whether the chip is urgent. All 5 write tools are `false`.
    pub urgent: bool,
}

/// The strict confirmation gate every write tool routes through BEFORE it
/// mutates (§4.8 / D4, `design/08 R-2`: no bypass).
///
/// The production gate ([`StrictResolverGate`]) consults a strict
/// [`PermissionResolver`] (⇒ `MustAsk` for these Restricted tools), emits the
/// `AwaitingApproval` event on the Maestro session, registers a
/// `oneshot::Sender<Decision>` in the supervisor's EXISTING
/// [`crate::agent_supervisor::PendingApprovals`] map (no second registry), and
/// parks until `Sessions.ResolveApproval` wakes it — exactly the Task 33 / 402
/// chip lifecycle. The live emit/park/resolve plumbing is supplied by the
/// boot-time spine (Task 414) via a [`ConfirmationSink`]; this file owns the
/// "decide under strict, then ask" policy.
///
/// Tests script the decision directly via a fake gate.
#[async_trait]
pub trait MaestroGate: Send + Sync {
    /// Decide `req` under the Maestro session's strict mode. Returns the
    /// terminal [`Decision`] (`AutoApprove` / `AutoApproveOnce` / `AutoDeny`);
    /// a `MustAsk` is resolved INSIDE the gate (it parks until the user
    /// answers) so callers never see `MustAsk`.
    async fn confirm(&self, req: GateRequest) -> Result<Decision, McpError>;
}

/// True iff `decision` permits the mutation to proceed. `AutoDeny` (and the
/// safety-default `MustAsk`, which a correct gate never returns) block it.
fn approved(decision: Decision) -> bool {
    matches!(decision, Decision::AutoApprove | Decision::AutoApproveOnce)
}

/// The typed `"user declined"` tool result returned (and NO mutation run) when
/// the gate resolves to deny. A structured error, not empty-success — the agent
/// sees the call was refused.
fn declined_err(tool: &str) -> McpError {
    McpError::invalid_params(
        format!("user declined the confirmation for {tool}; no mutation was performed"),
        None,
    )
}

/// The out-of-band confirmation-chip seam the production [`StrictResolverGate`]
/// drives once the resolver returns `MustAsk`: emit `AwaitingApproval` on the
/// Maestro session, register the `oneshot` in the supervisor's existing
/// `PendingApprovals`, and park on the user's `ResolveApproval`.
///
/// 401's MCP dispatch context carries the calling (Maestro) `SessionId`; the
/// boot spine (Task 414) supplies the concrete sink that bridges into the
/// supervisor. Kept as a trait so this file does not fork the approval registry
/// nor reach into `agent_supervisor/actor.rs` (outside 406's write-set).
#[async_trait]
pub trait ConfirmationSink: Send + Sync {
    /// Raise the confirmation chip for `req` on the Maestro session and block
    /// until the user resolves it, returning their [`Decision`]
    /// (`AutoApprove` / `AutoApproveOnce` / `AutoDeny`). A dropped session
    /// (sender gone) maps to `AutoDeny` (safe refusal).
    async fn ask_user(&self, req: &GateRequest) -> Result<Decision, McpError>;
}

/// The production gate: classify under a strict [`PermissionResolver`], and for
/// the `MustAsk` verdict drive the [`ConfirmationSink`] (the
/// `AwaitingApproval`/`ResolveApproval` chip). Auto-decisions (never produced by
/// strict for these Restricted tools, but handled for completeness) short-circuit
/// without a chip.
pub struct StrictResolverGate {
    resolver: PermissionResolver,
    sink: std::sync::Arc<dyn ConfirmationSink>,
}

impl StrictResolverGate {
    /// Build the gate from the Maestro session's strict resolver + the boot
    /// spine's confirmation sink (Task 414 wiring).
    pub fn new(resolver: PermissionResolver, sink: std::sync::Arc<dyn ConfirmationSink>) -> Self {
        Self { resolver, sink }
    }
}

#[async_trait]
impl MaestroGate for StrictResolverGate {
    async fn confirm(&self, req: GateRequest) -> Result<Decision, McpError> {
        // (1) decide under strict. For the 5 write tools (Restricted) this is
        // ALWAYS MustAsk; reads (ReadOnly) auto-approve but never reach here.
        match self.resolver.decide(req.tool) {
            Decision::MustAsk => self.sink.ask_user(&req).await,
            // A non-strict mode (or a future loosening) could auto-decide; honor
            // it without a chip. Under the Maestro's invariant strict mode this
            // arm is unreachable, but we never silently bypass a verdict.
            other => Ok(other),
        }
    }
}

// ===========================================================================
// Mutation seams — the manager surfaces the 5 tools drive.
//
// Reuse `routing::InputSink` (Task 408) for the send path. The create/transition
// surfaces are abstracted as narrow traits so `dispatch_write` is unit-testable
// against in-process fakes (the Tier-2 double) and the live impls bind to the
// real managers in the boot spine (Task 414).
// ===========================================================================

pub use crate::maestro::routing::InputSink;

/// The `create_workspace` mutation seam (the frozen
/// `WorkspaceManager::create_workspace` shape, mapped from a spec).
#[async_trait]
pub trait WorkspaceCreator: Send + Sync {
    /// Create a workspace, returning its new id. Maps onto
    /// `WorkspaceManager::create_workspace(name, &repos, permission_mode,
    /// description, icon)`.
    async fn create_workspace(&self, spec: WorkspaceSpec) -> concerto_error::Result<String>;
}

/// The `create_workarea` mutation seam (the frozen
/// `WorkareaManager::create_workarea` shape).
#[async_trait]
pub trait WorkareaCreator: Send + Sync {
    /// Create a workarea in `workspace_id`, returning its new id. Maps onto
    /// `WorkareaManager::create_workarea(workspace_id, permission_mode)`.
    async fn create_workarea(
        &self,
        workspace_id: &str,
        spec: WorkareaSpec,
    ) -> concerto_error::Result<String>;
}

/// The `set_workarea_paused` mutation seam (the frozen
/// `WorkareaManager::transition_workarea` shape).
#[async_trait]
pub trait WorkareaTransitioner: Send + Sync {
    /// Pause (`paused = true` ⇒ `WorkareaEvent::Pause`) or resume (`false` ⇒
    /// `Resume`) `workarea_id`. The FSM precondition (e.g. pausing an archived
    /// workarea) surfaces as `Error::Policy`.
    async fn set_paused(&self, workarea_id: &str, paused: bool) -> concerto_error::Result<()>;
}

/// The 5-tool mutation context: the seams `dispatch_write` drives once a gate
/// approves. Built by the boot spine (Task 414) from the live Core handles;
/// tests build it from in-process fakes.
pub struct WriteToolCtx<'a> {
    /// The strict confirmation gate (the §4.8 chip-gate). Drives BEFORE any
    /// mutation; a deny short-circuits with [`declined_err`].
    pub gate: &'a dyn MaestroGate,
    /// The send-prompt path (`AgentSupervisorHandle::send_input`, Task 408's
    /// `InputSink`). Used by `route_prompt_to_session` / `fanout_to_sessions`.
    pub sink: &'a dyn InputSink,
    /// The `create_workspace` seam.
    pub workspaces: &'a dyn WorkspaceCreator,
    /// The `create_workarea` seam.
    pub workareas: &'a dyn WorkareaCreator,
    /// The `set_workarea_paused` (transition) seam.
    pub transitions: &'a dyn WorkareaTransitioner,
}

// ===========================================================================
// The 5 write tools.
// ===========================================================================

/// `route_prompt_to_session(session_id, prompt)` → gate, then
/// `send_input(&session_id, prompt.into_bytes())`. Returns the frozen empty
/// success object. A non-existent / closed session surfaces as a typed error
/// (the `InputSink` returns `Error::NotFound`), not a panic, not empty-success.
pub async fn route_prompt_to_session(
    ctx: &WriteToolCtx<'_>,
    session_id: String,
    prompt: String,
) -> Result<Value, McpError> {
    let req = GateRequest {
        tool: "route_prompt_to_session",
        summary: format!("Route a prompt to session {session_id}"),
        destructive_label: None,
        urgent: false,
    };
    // GATE FIRST.
    let decision = ctx.gate.confirm(req).await?;
    if !approved(decision) {
        return Err(declined_err("route_prompt_to_session"));
    }
    // MUTATE SECOND.
    let sid = SessionId(session_id);
    ctx.sink
        .send_input(&sid, prompt.into_bytes())
        .await
        .map_err(map_err)?;
    Ok(json!({}))
}

/// `fanout_to_sessions([session_ids], prompt)` → ONE gate for the whole fanout
/// (`design/08 §5.1`: the user confirms "send to N sessions" once, not N chips),
/// then `send_input` per target. Collects per-session ok/err so a single closed
/// target is reported in the result rather than failing the whole call or being
/// swallowed.
pub async fn fanout_to_sessions(
    ctx: &WriteToolCtx<'_>,
    session_ids: Vec<String>,
    prompt: String,
) -> Result<Value, McpError> {
    let req = GateRequest {
        tool: "fanout_to_sessions",
        summary: format!("Fan a prompt out to {} session(s)", session_ids.len()),
        destructive_label: None,
        urgent: false,
    };
    // ONE GATE for the whole fanout.
    let decision = ctx.gate.confirm(req).await?;
    if !approved(decision) {
        return Err(declined_err("fanout_to_sessions"));
    }
    // MUTATE SECOND — per target, collecting per-session ok/err.
    let bytes = prompt.into_bytes();
    let mut results = Vec::with_capacity(session_ids.len());
    let mut all_ok = true;
    for raw in session_ids {
        let sid = SessionId(raw.clone());
        match ctx.sink.send_input(&sid, bytes.clone()).await {
            Ok(()) => results.push(json!({ "session_id": raw, "ok": true })),
            Err(e) => {
                all_ok = false;
                results.push(json!({ "session_id": raw, "ok": false, "error": e.to_string() }));
            }
        }
    }
    Ok(json!({ "results": results, "all_ok": all_ok }))
}

/// `set_workarea_paused(workarea_id, paused)` → gate, then
/// `transition_workarea(Pause|Resume)`. Reversible ⇒ `destructive_label = None`,
/// `urgent = false`. An illegal transition (e.g. pausing an archived workarea)
/// surfaces the FSM's typed `Error::Policy` as the tool's error.
pub async fn set_workarea_paused(
    ctx: &WriteToolCtx<'_>,
    workarea_id: String,
    paused: bool,
) -> Result<Value, McpError> {
    let verb = if paused { "Pause" } else { "Resume" };
    let req = GateRequest {
        tool: "set_workarea_paused",
        summary: format!("{verb} workarea {workarea_id}"),
        destructive_label: None,
        urgent: false,
    };
    // GATE FIRST.
    let decision = ctx.gate.confirm(req).await?;
    if !approved(decision) {
        return Err(declined_err("set_workarea_paused"));
    }
    // MUTATE SECOND.
    ctx.transitions
        .set_paused(&workarea_id, paused)
        .await
        .map_err(map_err)?;
    Ok(json!({}))
}

// ===========================================================================
// Create flow — the reusable inner fns Task 411 wraps (§8.1 "create flow"
// region; 411 adds its issue-parse / cone-suggest front-end and the privacy-debt
// fix in front of these, then calls them).
// ===========================================================================

/// The mapped `create_workspace` spec — the frozen `spec` object decoded into
/// the `WorkspaceManager::create_workspace` argument shape. Task 411 builds this
/// from an issue parse + cone-suggest, then calls [`do_create_workspace`].
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSpec {
    /// Workspace name (required by the manager; empty ⇒ `Error::Validation`).
    pub name: String,
    /// Attached repository ids (≥1 required by the manager). Empty cones ⇒
    /// seed-from-defaults.
    pub repository_ids: Vec<String>,
    /// Optional permission mode (`strict|normal|auto|yolo`); `None` ⇒ inherit.
    pub permission_mode: Option<String>,
    /// Optional description.
    pub description: Option<String>,
    /// Optional icon.
    pub icon: Option<String>,
}

impl WorkspaceSpec {
    /// Decode the frozen `spec` object (401's `create_workspace.spec`) into the
    /// manager arg shape. `repository_ids` is an array of strings; the other
    /// fields are optional. The manager validates non-empty name / ≥1 repo, so
    /// this mapping stays permissive (it never invents defaults the manager
    /// would not).
    pub fn from_spec(spec: &Value) -> Result<Self, McpError> {
        let name = opt_str(spec, "name").unwrap_or_default();
        let repository_ids = spec
            .get("repository_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            name,
            repository_ids,
            permission_mode: opt_str(spec, "permission_mode"),
            description: opt_str(spec, "description"),
            icon: opt_str(spec, "icon"),
        })
    }
}

/// The mapped `create_workarea` spec. Today the manager only takes an optional
/// `permission_mode`; the struct is the seam Task 411 extends (e.g. with a
/// composer hint) without changing the tool signature.
#[derive(Debug, Clone, Default)]
pub struct WorkareaSpec {
    /// Optional permission mode (`strict|normal|auto|yolo`); `None` ⇒ inherit.
    pub permission_mode: Option<String>,
}

impl WorkareaSpec {
    /// Decode the frozen `spec` object (401's `create_workarea.spec`).
    pub fn from_spec(spec: &Value) -> Result<Self, McpError> {
        Ok(Self {
            permission_mode: opt_str(spec, "permission_mode"),
        })
    }
}

/// **Reusable inner fn (Task 411 wraps this).** Create a workspace from an
/// already-mapped [`WorkspaceSpec`], returning the new `workspace_id`. Pure
/// mutation — the gate is applied by the [`create_workspace`] tool wrapper, so
/// 411 (which gates its own confirm-chips up front) calls this directly.
pub async fn do_create_workspace(
    ctx: &WriteToolCtx<'_>,
    spec: WorkspaceSpec,
) -> Result<String, McpError> {
    ctx.workspaces.create_workspace(spec).await.map_err(map_err)
}

/// **Reusable inner fn (Task 411 wraps this).** Create a workarea in
/// `workspace_id` from an already-mapped [`WorkareaSpec`], returning the new
/// `workarea_id`. Pure mutation — gate is applied by the [`create_workarea`]
/// tool wrapper.
pub async fn do_create_workarea(
    ctx: &WriteToolCtx<'_>,
    workspace_id: &str,
    spec: WorkareaSpec,
) -> Result<String, McpError> {
    ctx.workareas
        .create_workarea(workspace_id, spec)
        .await
        .map_err(map_err)
}

/// `create_workspace(spec) → { workspace_id }` → gate, then [`do_create_workspace`].
pub async fn create_workspace(ctx: &WriteToolCtx<'_>, spec: &Value) -> Result<Value, McpError> {
    let mapped = WorkspaceSpec::from_spec(spec)?;
    let req = GateRequest {
        tool: "create_workspace",
        summary: if mapped.name.is_empty() {
            "Create a workspace".to_string()
        } else {
            format!("Create workspace {:?}", mapped.name)
        },
        destructive_label: None,
        urgent: false,
    };
    // GATE FIRST.
    let decision = ctx.gate.confirm(req).await?;
    if !approved(decision) {
        return Err(declined_err("create_workspace"));
    }
    // MUTATE SECOND (via the reusable inner fn 411 also calls).
    let workspace_id = do_create_workspace(ctx, mapped).await?;
    Ok(json!({ "workspace_id": workspace_id }))
}

/// `create_workarea(workspace_id, spec) → { workarea_id }` → gate, then
/// [`do_create_workarea`].
pub async fn create_workarea(
    ctx: &WriteToolCtx<'_>,
    workspace_id: String,
    spec: &Value,
) -> Result<Value, McpError> {
    let mapped = WorkareaSpec::from_spec(spec)?;
    let req = GateRequest {
        tool: "create_workarea",
        summary: format!("Create a workarea in workspace {workspace_id}"),
        destructive_label: None,
        urgent: false,
    };
    // GATE FIRST.
    let decision = ctx.gate.confirm(req).await?;
    if !approved(decision) {
        return Err(declined_err("create_workarea"));
    }
    // MUTATE SECOND (via the reusable inner fn 411 also calls).
    let workarea_id = do_create_workarea(ctx, &workspace_id, mapped).await?;
    Ok(json!({ "workarea_id": workarea_id }))
}

// ===========================================================================
// Argument-deserializing entry point (the frozen 401 arg sets).
// ===========================================================================

/// Dispatch a write tool by its frozen name, deserializing `args` per 401's
/// frozen input schema, driving the strict gate, then the mutation, and
/// returning the frozen output JSON.
///
/// This is the seam the live MCP server (`super::super::mcp`, once Task 414
/// threads the Core handles + the confirmation sink into `MaestroMcpServer`)
/// calls in place of 401's typed-unimplemented arm for the 5 write tools. Until
/// that boot wiring lands, the handle-less sync [`super::dispatch`] keeps 401's
/// typed seam error (never a macro, never a fake-success) — `dispatch_write` is
/// the route that actually runs.
pub async fn dispatch_write(
    name: &str,
    args: Option<Map<String, Value>>,
    ctx: &WriteToolCtx<'_>,
) -> Result<Value, McpError> {
    match name {
        "route_prompt_to_session" => {
            route_prompt_to_session(
                ctx,
                req_str(&args, "session_id")?,
                req_str(&args, "prompt")?,
            )
            .await
        }
        "fanout_to_sessions" => {
            fanout_to_sessions(
                ctx,
                req_str_array(&args, "session_ids")?,
                req_str(&args, "prompt")?,
            )
            .await
        }
        "create_workspace" => create_workspace(ctx, req_obj(&args, "spec")?).await,
        "create_workarea" => {
            let workspace_id = req_str(&args, "workspace_id")?;
            create_workarea(ctx, workspace_id, req_obj(&args, "spec")?).await
        }
        "set_workarea_paused" => {
            set_workarea_paused(
                ctx,
                req_str(&args, "workarea_id")?,
                req_bool(&args, "paused")?,
            )
            .await
        }
        other => Err(McpError::invalid_params(
            format!("not a maestro write tool: {other}"),
            None,
        )),
    }
}

// ===========================================================================
// Live impls — bind the seams to the real Core handles (the boot spine, Task
// 414, constructs a `WriteToolCtx` from these). `InputSink for
// AgentSupervisorHandle` already lives in `routing.rs` (Task 408); here we add
// the create/transition impls over the managers.
// ===========================================================================

#[async_trait]
impl WorkspaceCreator for crate::workspace_manager::WorkspaceManager {
    async fn create_workspace(&self, spec: WorkspaceSpec) -> concerto_error::Result<String> {
        let repos: Vec<crate::workspace_manager::WorkspaceRepoSpec> = spec
            .repository_ids
            .iter()
            .map(|rid| crate::workspace_manager::WorkspaceRepoSpec {
                repository_id: concerto_persist::RepositoryId(rid.clone()),
                sparse_cones: Vec::new(),
            })
            .collect();
        let ws = crate::workspace_manager::WorkspaceManager::create_workspace(
            self,
            &spec.name,
            &repos,
            spec.permission_mode,
            spec.description,
            spec.icon,
        )
        .await?;
        Ok(ws.id.0)
    }
}

#[async_trait]
impl WorkareaCreator for crate::workspace_manager::WorkareaManager {
    async fn create_workarea(
        &self,
        workspace_id: &str,
        spec: WorkareaSpec,
    ) -> concerto_error::Result<String> {
        let wa = crate::workspace_manager::WorkareaManager::create_workarea(
            self,
            workspace_id,
            spec.permission_mode,
        )
        .await?;
        Ok(wa.id.0)
    }
}

#[async_trait]
impl WorkareaTransitioner for crate::workspace_manager::WorkareaManager {
    async fn set_paused(&self, workarea_id: &str, paused: bool) -> concerto_error::Result<()> {
        use crate::workspace_manager::fsm::WorkareaEvent;
        let event = if paused {
            WorkareaEvent::Pause
        } else {
            WorkareaEvent::Resume
        };
        crate::workspace_manager::WorkareaManager::transition_workarea(
            self,
            &concerto_persist::WorkareaId(workarea_id.to_string()),
            event,
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    // ---- scripted fakes (the Tier-2 in-process double) --------------------

    /// A gate that records every confirmation it was asked for and returns a
    /// scripted decision. Proves the §4.8 invariant: a tool MUST call the gate
    /// before it mutates.
    struct ScriptedGate {
        decision: Decision,
        seen: Mutex<Vec<GateRequest>>,
    }
    impl ScriptedGate {
        fn new(decision: Decision) -> Self {
            Self {
                decision,
                seen: Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
        fn last_tool(&self) -> Option<&'static str> {
            self.seen.lock().unwrap().last().map(|r| r.tool)
        }
    }
    #[async_trait]
    impl MaestroGate for ScriptedGate {
        async fn confirm(&self, req: GateRequest) -> Result<Decision, McpError> {
            self.seen.lock().unwrap().push(req);
            Ok(self.decision)
        }
    }

    /// A send-sink that records the (session_id, bytes) of every send and can be
    /// scripted to fail for specific session ids (a "closed session").
    #[derive(Default)]
    struct RecordingSink {
        sends: Mutex<Vec<(String, Vec<u8>)>>,
        closed: Mutex<Vec<String>>,
    }
    impl RecordingSink {
        fn close(&self, sid: &str) {
            self.closed.lock().unwrap().push(sid.to_string());
        }
        fn sends(&self) -> Vec<(String, Vec<u8>)> {
            self.sends.lock().unwrap().clone()
        }
    }
    #[async_trait]
    impl InputSink for RecordingSink {
        async fn send_input(
            &self,
            session_id: &SessionId,
            data: Vec<u8>,
        ) -> concerto_error::Result<()> {
            if self
                .closed
                .lock()
                .unwrap()
                .iter()
                .any(|s| s == &session_id.0)
            {
                return Err(concerto_error::Error::NotFound(format!(
                    "session {} not running",
                    session_id.0
                )));
            }
            self.sends
                .lock()
                .unwrap()
                .push((session_id.0.clone(), data));
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingCreators {
        created_ws: Mutex<Vec<WorkspaceSpec>>,
        created_wa: Mutex<Vec<(String, WorkareaSpec)>>,
        transitions: Mutex<Vec<(String, bool)>>,
        /// When set, `set_paused` returns this FSM policy error (illegal pause).
        pause_policy_err: AtomicBool,
    }
    #[async_trait]
    impl WorkspaceCreator for RecordingCreators {
        async fn create_workspace(&self, spec: WorkspaceSpec) -> concerto_error::Result<String> {
            self.created_ws.lock().unwrap().push(spec);
            Ok("ws-new".to_string())
        }
    }
    #[async_trait]
    impl WorkareaCreator for RecordingCreators {
        async fn create_workarea(
            &self,
            workspace_id: &str,
            spec: WorkareaSpec,
        ) -> concerto_error::Result<String> {
            self.created_wa
                .lock()
                .unwrap()
                .push((workspace_id.to_string(), spec));
            Ok("wa-new".to_string())
        }
    }
    #[async_trait]
    impl WorkareaTransitioner for RecordingCreators {
        async fn set_paused(&self, workarea_id: &str, paused: bool) -> concerto_error::Result<()> {
            if self.pause_policy_err.load(Ordering::SeqCst) {
                // Mirror `transition_workarea`'s typed `Error::Policy` for an
                // illegal transition (e.g. pausing an archived workarea).
                return Err(concerto_error::Error::Policy(
                    "workarea.invalid_transition: cannot pause an archived workarea".into(),
                ));
            }
            self.transitions
                .lock()
                .unwrap()
                .push((workarea_id.to_string(), paused));
            Ok(())
        }
    }

    /// Assemble a `WriteToolCtx` from the shared fakes. Returns owned fakes so
    /// the test can assert on them after the call.
    fn ctx<'a>(
        gate: &'a ScriptedGate,
        sink: &'a RecordingSink,
        creators: &'a RecordingCreators,
    ) -> WriteToolCtx<'a> {
        WriteToolCtx {
            gate,
            sink,
            workspaces: creators,
            workareas: creators,
            transitions: creators,
        }
    }

    // ---- (1) route: scripted-approve sends the exact bytes ----------------

    #[tokio::test]
    async fn route_approve_sends_exact_bytes_after_gate() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let out = route_prompt_to_session(&ctx, "sess-1".into(), "run the suite".into())
            .await
            .expect("approved route succeeds");
        assert_eq!(out, json!({}));

        // Gate fired exactly once, BEFORE the send.
        assert_eq!(gate.calls(), 1);
        assert_eq!(gate.last_tool(), Some("route_prompt_to_session"));
        // The exact prompt bytes reached the one session.
        assert_eq!(
            sink.sends(),
            vec![("sess-1".to_string(), b"run the suite".to_vec())]
        );
    }

    // ---- (2) route: scripted-deny does NOT send ---------------------------

    #[tokio::test]
    async fn route_deny_declines_and_never_sends() {
        let gate = ScriptedGate::new(Decision::AutoDeny);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let err = route_prompt_to_session(&ctx, "sess-1".into(), "nope".into())
            .await
            .expect_err("denied route returns a typed declined error");
        assert!(err.message.contains("user declined"));
        // Gate fired; send NEVER happened.
        assert_eq!(gate.calls(), 1);
        assert!(sink.sends().is_empty(), "deny must not mutate");
    }

    // ---- (3) fanout: one gate, N sends, per-session error on a closed target

    #[tokio::test]
    async fn fanout_one_gate_two_sends_partial_failure_reported() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        sink.close("sess-closed"); // second target is closed
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let out = fanout_to_sessions(
            &ctx,
            vec!["sess-ok".into(), "sess-closed".into()],
            "ping".into(),
        )
        .await
        .expect("fanout returns a result even with a partial failure");

        // ONE gate for the whole fanout (not N).
        assert_eq!(gate.calls(), 1, "fanout gates exactly once");
        // The open session got the bytes; the closed one did not.
        assert_eq!(
            sink.sends(),
            vec![("sess-ok".to_string(), b"ping".to_vec())]
        );
        // The result reports the per-session ok/err (no swallow).
        assert_eq!(out["all_ok"], json!(false));
        let results = out["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["session_id"], json!("sess-ok"));
        assert_eq!(results[0]["ok"], json!(true));
        assert_eq!(results[1]["session_id"], json!("sess-closed"));
        assert_eq!(results[1]["ok"], json!(false));
        assert!(results[1].get("error").is_some());
    }

    #[tokio::test]
    async fn fanout_deny_never_sends() {
        let gate = ScriptedGate::new(Decision::AutoDeny);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let err = fanout_to_sessions(&ctx, vec!["a".into(), "b".into()], "x".into())
            .await
            .expect_err("denied fanout declines");
        assert!(err.message.contains("user declined"));
        assert!(sink.sends().is_empty());
    }

    // ---- (4) create: approve returns real ids; the seam was called --------

    #[tokio::test]
    async fn create_workspace_approve_returns_id_and_calls_manager() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let spec = json!({ "name": "Maestro WS", "repository_ids": ["repo-1"] });
        let out = create_workspace(&ctx, &spec).await.expect("create ok");
        assert_eq!(out, json!({ "workspace_id": "ws-new" }));
        assert_eq!(gate.calls(), 1);
        let created = creators.created_ws.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].name, "Maestro WS");
        assert_eq!(created[0].repository_ids, vec!["repo-1".to_string()]);
    }

    #[tokio::test]
    async fn create_workarea_approve_returns_id_and_calls_manager() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let spec = json!({ "permission_mode": "strict" });
        let out = create_workarea(&ctx, "ws-1".into(), &spec)
            .await
            .expect("create ok");
        assert_eq!(out, json!({ "workarea_id": "wa-new" }));
        let created = creators.created_wa.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0, "ws-1");
        assert_eq!(created[0].1.permission_mode.as_deref(), Some("strict"));
    }

    #[tokio::test]
    async fn create_workspace_deny_does_not_create() {
        let gate = ScriptedGate::new(Decision::AutoDeny);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let spec = json!({ "name": "WS", "repository_ids": ["r"] });
        let err = create_workspace(&ctx, &spec)
            .await
            .expect_err("denied create declines");
        assert!(err.message.contains("user declined"));
        assert!(creators.created_ws.lock().unwrap().is_empty());
    }

    // ---- (5) pause/resume round-trip + illegal pause = typed Error::Policy -

    #[tokio::test]
    async fn pause_then_resume_round_trip() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        set_workarea_paused(&ctx, "wa-1".into(), true)
            .await
            .expect("pause ok");
        set_workarea_paused(&ctx, "wa-1".into(), false)
            .await
            .expect("resume ok");

        let t = creators.transitions.lock().unwrap();
        assert_eq!(
            *t,
            vec![("wa-1".to_string(), true), ("wa-1".to_string(), false)]
        );
    }

    #[tokio::test]
    async fn illegal_pause_surfaces_typed_policy_error() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        creators.pause_policy_err.store(true, Ordering::SeqCst);
        let ctx = ctx(&gate, &sink, &creators);

        let err = set_workarea_paused(&ctx, "wa-archived".into(), true)
            .await
            .expect_err("illegal pause is a typed tool error, not a panic");
        // The FSM `Error::Policy` maps to a typed invalid_params (rejected), and
        // carries the wire code.
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("invalid_transition"));
    }

    // ---- (6) every write tool gates BEFORE any mutation (the §4.8 invariant)

    #[tokio::test]
    async fn every_write_tool_gates_before_mutating() {
        // Deny everything; assert no mutation reached any seam for any of the 5.
        let gate = ScriptedGate::new(Decision::AutoDeny);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        let _ = dispatch_write(
            "route_prompt_to_session",
            Some(
                json!({ "session_id": "s", "prompt": "p" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            &ctx,
        )
        .await;
        let _ = dispatch_write(
            "fanout_to_sessions",
            Some(
                json!({ "session_ids": ["s"], "prompt": "p" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            &ctx,
        )
        .await;
        let _ = dispatch_write(
            "create_workspace",
            Some(
                json!({ "spec": { "name": "n", "repository_ids": ["r"] } })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            &ctx,
        )
        .await;
        let _ = dispatch_write(
            "create_workarea",
            Some(
                json!({ "workspace_id": "w", "spec": {} })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            &ctx,
        )
        .await;
        let _ = dispatch_write(
            "set_workarea_paused",
            Some(
                json!({ "workarea_id": "w", "paused": true })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            &ctx,
        )
        .await;

        // The gate fired for all 5; NO seam mutated (deny dominated every call).
        assert_eq!(gate.calls(), 5, "all 5 write tools hit the gate");
        assert!(sink.sends().is_empty());
        assert!(creators.created_ws.lock().unwrap().is_empty());
        assert!(creators.created_wa.lock().unwrap().is_empty());
        assert!(creators.transitions.lock().unwrap().is_empty());
    }

    // ---- the production gate computes MustAsk under strict, then drives the
    // confirmation sink (the chip) — proving strict ⇒ chip, no bypass ----------

    #[tokio::test]
    async fn strict_resolver_gate_must_ask_drives_the_confirmation_sink() {
        use crate::security::PermissionMode;

        struct ScriptedSink {
            decision: Decision,
            asked: AtomicBool,
        }
        #[async_trait]
        impl ConfirmationSink for ScriptedSink {
            async fn ask_user(&self, _req: &GateRequest) -> Result<Decision, McpError> {
                self.asked.store(true, Ordering::SeqCst);
                Ok(self.decision)
            }
        }

        let sink = Arc::new(ScriptedSink {
            decision: Decision::AutoApprove,
            asked: AtomicBool::new(false),
        });
        let resolver = PermissionResolver::new(PermissionMode::Strict, false);
        let gate = StrictResolverGate::new(resolver, sink.clone());

        // A write tool under strict ⇒ MustAsk ⇒ the chip (sink) is consulted.
        let decision = gate
            .confirm(GateRequest {
                tool: "create_workspace",
                summary: "x".into(),
                destructive_label: None,
                urgent: false,
            })
            .await
            .expect("gate ok");
        assert!(
            sink.asked.load(Ordering::SeqCst),
            "strict must raise the chip"
        );
        assert_eq!(decision, Decision::AutoApprove);
    }

    #[tokio::test]
    async fn strict_resolver_gate_deny_blocks_the_tool() {
        use crate::security::PermissionMode;

        struct DenySink;
        #[async_trait]
        impl ConfirmationSink for DenySink {
            async fn ask_user(&self, _req: &GateRequest) -> Result<Decision, McpError> {
                Ok(Decision::AutoDeny)
            }
        }
        let resolver = PermissionResolver::new(PermissionMode::Strict, false);
        let gate = StrictResolverGate::new(resolver, Arc::new(DenySink));
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = WriteToolCtx {
            gate: &gate,
            sink: &sink,
            workspaces: &creators,
            workareas: &creators,
            transitions: &creators,
        };

        let err = route_prompt_to_session(&ctx, "s".into(), "p".into())
            .await
            .expect_err("strict-deny blocks the tool");
        assert!(err.message.contains("user declined"));
        assert!(sink.sends().is_empty());
    }

    // ---- arg validation: missing/malformed frozen args ⇒ typed invalid_params

    #[tokio::test]
    async fn dispatch_rejects_unknown_and_malformed_args() {
        let gate = ScriptedGate::new(Decision::AutoApprove);
        let sink = RecordingSink::default();
        let creators = RecordingCreators::default();
        let ctx = ctx(&gate, &sink, &creators);

        // Unknown tool.
        let err = dispatch_write("not_a_tool", None, &ctx).await.unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);

        // Missing required arg.
        let err = dispatch_write("route_prompt_to_session", None, &ctx)
            .await
            .unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        // No mutation on a malformed call.
        assert!(sink.sends().is_empty());
    }
}

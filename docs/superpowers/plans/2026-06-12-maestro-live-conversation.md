# Maestro Live Conversation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Maestro chat actually converse — type a message, get a live grounded reply rendered as chat bubbles — by switching the Maestro session from interactive TUI to structured `stream-json` I/O and bridging its turns to the `maestro.events` stream the chat UI already renders.

**Architecture:** Reuse the agent-host session lifecycle. The Maestro provider launches `claude` in headless `stream-json` mode; `forward_freeform` sends a structured user-message envelope; a new `MaestroStreamJsonPack` parses Claude's JSON event lines into `ParseEvent::Message`/`TurnComplete`; a per-session **events bridge** re-emits those as `MaestroEvent::Message` on `maestro.events`; the user turn and history persist to `chat_messages`.

**Tech Stack:** Rust (tokio, serde_json, tonic/prost, sqlx/SQLite), the Concerto agent-supervisor + maestro crates, the Tauri desktop (TS/React).

**Reference spec:** `docs/superpowers/specs/2026-06-12-maestro-live-conversation-design.md`

**Branch:** `maestro-live-conversation` (stacked on `maestro-live-integration` / PR #178).

**Scope:** Milestone-1 conversation only. Write MCP tools + confirmation chips and the full `--permission-prompt-tool` flow are M2. The digest summarizer is a separate follow-up. Read tools auto-approve via `--allowedTools`.

---

## Background facts (verified in the codebase — read before starting)

- **Parser pack trait** (`crates/core/src/agent_supervisor/parsers/mod.rs`): `trait ParserPack { fn agent_kind(&self)->AgentKind; fn version_pattern(&self)->&str; fn parse_chunk(&self, buf:&mut Vec<u8>)->Vec<ParseEvent>; fn inject_approval(&self, d:Decision)->Vec<u8>; }`. `parse_chunk` MAY hold bytes back in `buf` for partial-line accumulation.
- **`ParseEvent`** variants: `Bytes(Vec<u8>)`, `Message{role:MsgRole, content:String}`, `ToolCall{name,args,call_id}`, `AwaitingApproval{...}`, `TurnComplete`. `#[non_exhaustive]`. `MsgRole::{User,Assistant,System,Tool}`.
- **Read-pump** (`agent_supervisor/actor.rs` ~2181-2249): `ParseEvent::Message{role,content}` → `AgentEvent::Message{session_id, role, content}` on the supervisor `events` broadcast; `ParseEvent::TurnComplete` → `AgentEvent::TurnComplete` + `checkpoint::insert_turn_message` (persists an `assistant` `chat_messages` row).
- **Maestro events** (`crates/core/src/maestro/events.rs`): `MaestroEvent::Message{text:String, message_id:String}` exists, serializes to `{"kind":"maestro.message","text":…,"message_id":…}`, emitted via `MaestroEventSender::emit(MaestroEvent)`. `maestro.events` has a live producer (boot wires `handle.events_sender()`). **Nothing currently emits `MaestroEvent::Message`.**
- **Supervisor subscribe** (`actor.rs:358`): `AgentSupervisorHandle::subscribe_events(&SessionId) -> Option<broadcast::Receiver<AgentEvent>>` (and `subscribe_events_with_replay`).
- **`forward_freeform`** (`maestro/handle.rs`): `send_input(&maestro_session_id(), body.as_bytes().to_vec())` — raw, no newline.
- **Provider args** (`maestro/provider.rs::resolve_cli_launch_spec`): builds `args = vec!["--model", model, "--mcp-config", path, "--strict-mcp-config", "--append-system-prompt", PREAMBLE]`.
- **Trust preseed precedent**: `agent_supervisor::ensure_claude_trusts_dir(cwd)` writes `~/.claude.json` → `projects.<canonical-cwd>.hasTrustDialogAccepted=true`.
- **`MaestroHandle` inner fields**: `inner.persistence: Arc<Persistence>`, `inner.supervisor: AgentSupervisorHandle`, `inner.events: MaestroEventSender`.
- **MaestroChat.tsx** subscribes to `MAESTRO_EVENTS_SUBJECT="maestro.events"`, decodes frames, renders `{kind:"message"... }` as bubbles via `MaestroTranscript`.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/core/tests/fixtures/maestro_stream_json/turn.jsonl` | A captured/synthetic Claude `stream-json` transcript fixture | Create |
| `crates/core/src/maestro/provider.rs` | Add the `stream-json` + `--allowedTools` flags to the launch spec | Modify |
| `crates/core/src/maestro/mcp_trust.rs` | `ensure_maestro_mcp_trusted()` — preseed `enabledMcpjsonServers` | Create |
| `crates/core/src/maestro/handle.rs` | `compose_user_envelope`; `forward_freeform` sends the envelope; persist user turn; spawn the events bridge | Modify |
| `crates/core/src/agent_supervisor/parsers/maestro_stream_json.rs` | `MaestroStreamJsonPack` — the `stream-json` parser | Create |
| `crates/core/src/agent_supervisor/parsers/mod.rs` + pack selection site | Register the new pack; select it for `AgentKind::Maestro` | Modify |
| `crates/core/src/maestro/events_bridge.rs` | `spawn_maestro_events_bridge` — session `AgentEvent::Message`/`TurnComplete` → `MaestroEvent::Message` | Create |
| `crates/persist/src/chat_messages.rs` | `list_by_chat(pool, chat_id, limit)` | Modify |
| `crates/proto/.../maestro.proto` + `handlers/maestro.rs` | `GetHistory` RPC returning the maestro chat turns | Modify |
| `apps/desktop/src/api/maestro.ts` + `MaestroChat.tsx` | Load history on mount; render persisted turns | Modify |

---

## Task 0: Spike — `stream-json` fixture (de-risk the parser)

**Files:**
- Create: `crates/core/tests/fixtures/maestro_stream_json/turn.jsonl`

The parser (Task 4) is table-tested against a fixture of Claude `stream-json` output. To avoid burning the user's Claude quota in the build loop, hand-author a fixture from the **known Claude Code `stream-json` schema** (one assistant turn that calls a read tool, then answers). Tier-3 later confirms it matches real output; if a real capture is cheaply available, replace the fixture with it.

- [ ] **Step 1: Create the fixture** (newline-delimited JSON, one object per line):

```jsonl
{"type":"system","subtype":"init","session_id":"s-1","tools":["mcp__concerto-maestro-mcp__list_workspaces"]}
{"type":"assistant","message":{"id":"msg_1","role":"assistant","content":[{"type":"text","text":"Let me check your workspaces."}]},"session_id":"s-1"}
{"type":"assistant","message":{"id":"msg_1","role":"assistant","content":[{"type":"tool_use","id":"tu_1","name":"mcp__concerto-maestro-mcp__list_workspaces","input":{}}]},"session_id":"s-1"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"[{\"id\":\"ws-real\",\"name\":\"Real\"}]"}]},"session_id":"s-1"}
{"type":"assistant","message":{"id":"msg_2","role":"assistant","content":[{"type":"text","text":"You have 1 workspace, \"Real\", with no active workareas."}]},"session_id":"s-1"}
{"type":"result","subtype":"success","is_error":false,"result":"You have 1 workspace, \"Real\", with no active workareas.","session_id":"s-1"}
```

- [ ] **Step 2: Commit**

```bash
git add crates/core/tests/fixtures/maestro_stream_json/turn.jsonl
git commit -m "test(maestro): stream-json transcript fixture for the conversation parser"
```

**Executor note:** if you can run Claude once cheaply (one short prompt) to capture a real transcript, prefer that and overwrite the fixture. Otherwise the synthetic fixture above (built from the documented `--output-format stream-json` schema) is the contract; the parser in Task 4 is written to it, and Task 8/Tier-3 validates against live Claude.

---

## Task 1: Provider — launch `claude` in `stream-json` mode

**Files:**
- Modify: `crates/core/src/maestro/provider.rs` (`resolve_cli_launch_spec`)
- Test: in-file `#[cfg(test)]` (extend `claude_provider_emits_mcp_strict_and_model_without_skip_permissions`)

- [ ] **Step 1: Write the failing test** (add to the provider tests):

```rust
#[test]
fn maestro_launch_is_headless_stream_json_with_read_tools_allowed() {
    let spec = ClaudeCliProvider::new()
        .resolve_launch(&ctx_with(ManagedPolicy::default()))
        .expect("spec");
    // Headless structured I/O — no interactive TUI.
    assert!(spec.args.windows(2).any(|w| w == ["--input-format", "stream-json"]));
    assert!(spec.args.windows(2).any(|w| w == ["--output-format", "stream-json"]));
    assert!(spec.args.iter().any(|a| a == "--verbose"));
    // Read tools auto-approve (M1); writes are inert.
    assert!(spec.args.windows(2).any(|w| w == ["--allowedTools", "mcp__concerto-maestro-mcp"]));
    // Still strict MCP + no skip-permissions.
    assert!(spec.args.iter().any(|a| a == "--strict-mcp-config"));
    assert!(!spec.args.iter().any(|a| a == "--dangerously-skip-permissions"));
}
```

- [ ] **Step 2: Run, verify FAIL**

Run: `cargo test -p concerto-core maestro::provider::tests::maestro_launch_is_headless`
Expected: FAIL (flags absent).

- [ ] **Step 3: Add the flags** in `resolve_cli_launch_spec`, replacing the `args` vec:

```rust
let args = vec![
    "--model".to_string(),
    model.clone(),
    "--input-format".to_string(),
    "stream-json".to_string(),
    "--output-format".to_string(),
    "stream-json".to_string(),
    "--verbose".to_string(),
    "--mcp-config".to_string(),
    mcp_config,
    "--strict-mcp-config".to_string(),
    // M1: auto-approve the live read tools (whole server; the 5 write + 2
    // side-channel tools return typed-unimplemented, so nothing to gate yet).
    // M2 replaces this with --permission-prompt-tool → PermissionResolver chips.
    "--allowedTools".to_string(),
    "mcp__concerto-maestro-mcp".to_string(),
    "--append-system-prompt".to_string(),
    MAESTRO_PREAMBLE.to_string(),
];
```
Update the existing test `claude_provider_emits_mcp_strict_and_model_without_skip_permissions` if it asserts an exact arg ordering/length (it checks `any(|a| ...)`, so it should still pass — verify).

- [ ] **Step 4: Run, verify PASS** — `cargo test -p concerto-core maestro::provider` → all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/provider.rs
git commit -m "feat(maestro): launch claude headless stream-json + allowedTools (read auto-approve)"
```

---

## Task 2: MCP-server trust preseed

**Files:**
- Create: `crates/core/src/maestro/mcp_trust.rs`
- Modify: `crates/core/src/maestro/mod.rs` (`pub mod mcp_trust;` + re-export), `handle.rs::spawn_maestro_session` (call it)
- Test: in-file `#[cfg(test)]`

Mirror `ensure_claude_trusts_dir` but for MCP-server approval: write `enabledMcpjsonServers:["concerto-maestro-mcp"]` into `~/.claude.json` `projects.<scratch>` so Claude never gates the server.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn preseed_adds_server_to_enabled_list_idempotently() {
    let home = tempfile::tempdir().unwrap();
    let scratch = home.path().join("concerto/maestro");
    std::fs::create_dir_all(&scratch).unwrap();
    let cfg = home.path().join(".claude.json");
    ensure_mcp_trusted_at(&cfg, &scratch).unwrap();
    ensure_mcp_trusted_at(&cfg, &scratch).unwrap(); // idempotent
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
    let key = std::fs::canonicalize(&scratch).unwrap().to_string_lossy().into_owned();
    let arr = v["projects"][&key]["enabledMcpjsonServers"].as_array().unwrap();
    assert_eq!(arr.iter().filter(|s| *s == "concerto-maestro-mcp").count(), 1);
}
```

- [ ] **Step 2: Run, verify FAIL** — `cargo test -p concerto-core maestro::mcp_trust` (function undefined).

- [ ] **Step 3: Implement** `crates/core/src/maestro/mcp_trust.rs`:

```rust
//! Pre-seed Claude's per-project MCP-server approval so the Maestro session
//! never blocks on the interactive "trust this MCP server?" gate. Mirrors
//! `agent_supervisor::ensure_claude_trusts_dir` (the folder-trust preseed) but
//! targets `projects.<scratch>.enabledMcpjsonServers`.

use std::path::Path;
use concerto_error::{Error, Result};
use crate::maestro::mcp::SERVER_NAME;

/// Preseed the MCP-server trust for the Maestro scratch project. Idempotent.
pub fn ensure_maestro_mcp_trusted(scratch_cwd: &Path) -> Result<()> {
    let home = home::home_dir()
        .ok_or_else(|| Error::Internal("cannot resolve home dir for ~/.claude.json".into()))?;
    ensure_mcp_trusted_at(&home.join(".claude.json"), scratch_cwd)
}

/// Testable core: take the config path explicitly.
pub fn ensure_mcp_trusted_at(config_path: &Path, scratch_cwd: &Path) -> Result<()> {
    let key = std::fs::canonicalize(scratch_cwd)
        .unwrap_or_else(|_| scratch_cwd.to_path_buf())
        .to_string_lossy()
        .into_owned();

    let mut root: serde_json::Value = match std::fs::read(config_path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| Error::Internal(format!("parse ~/.claude.json: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(Default::default())
        }
        Err(e) => return Err(Error::Io(e)),
    };
    if !root.is_object() {
        return Err(Error::Internal("~/.claude.json is not an object".into()));
    }
    let proj = root
        .as_object_mut().unwrap()
        .entry("projects").or_insert_with(|| serde_json::Value::Object(Default::default()))
        .as_object_mut().ok_or_else(|| Error::Internal("`projects` not an object".into()))?
        .entry(key).or_insert_with(|| serde_json::Value::Object(Default::default()));
    let obj = proj.as_object_mut().ok_or_else(|| Error::Internal("project entry not an object".into()))?;
    let list = obj
        .entry("enabledMcpjsonServers").or_insert_with(|| serde_json::Value::Array(Vec::new()))
        .as_array_mut().ok_or_else(|| Error::Internal("enabledMcpjsonServers not an array".into()))?;
    if !list.iter().any(|s| s == SERVER_NAME) {
        list.push(serde_json::Value::String(SERVER_NAME.to_string()));
    }
    std::fs::write(config_path, serde_json::to_vec_pretty(&root).expect("serialize"))
        .map_err(Error::Io)
}
```
Add `pub mod mcp_trust;` to `maestro/mod.rs` and re-export `ensure_maestro_mcp_trusted`. In `handle.rs::spawn_maestro_session`, after `ensure_maestro_scratch_dir()`/`ensure_maestro_scratch_trusted`, add a best-effort call:
```rust
let _ = crate::maestro::ensure_maestro_mcp_trusted(&scratch); // best-effort; logged by caller chain
```

- [ ] **Step 4: Run, verify PASS** — `cargo test -p concerto-core maestro::mcp_trust` + `cargo build -p concerto-core`.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/mcp_trust.rs crates/core/src/maestro/mod.rs crates/core/src/maestro/handle.rs
git commit -m "feat(maestro): preseed claude MCP-server trust at spawn (no interactive gate)"
```

---

## Task 3: Input framing — `stream-json` user envelope

**Files:**
- Modify: `crates/core/src/maestro/handle.rs` (`compose_user_envelope`, `forward_freeform`)
- Test: in-file `#[cfg(test)]`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn user_envelope_is_newline_terminated_stream_json() {
    let line = compose_user_envelope("what are my workareas doing?");
    assert!(line.ends_with('\n'));
    let v: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(v["type"], "user");
    assert_eq!(v["message"]["role"], "user");
    assert_eq!(v["message"]["content"][0]["type"], "text");
    assert_eq!(v["message"]["content"][0]["text"], "what are my workareas doing?");
}
```

- [ ] **Step 2: Run, verify FAIL** — `cargo test -p concerto-core maestro::handle::tests::user_envelope`.

- [ ] **Step 3: Implement** in `handle.rs`:

```rust
/// Frame a freeform user message as a Claude `stream-json` input line.
/// (`--input-format stream-json` reads one JSON object per line.)
pub(crate) fn compose_user_envelope(body: &str) -> String {
    let v = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": body }] }
    });
    format!("{}\n", serde_json::to_string(&v).expect("serialize user envelope"))
}
```
Change `forward_freeform`'s `send_input` to send the envelope:
```rust
async fn forward_freeform(&self, body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Ok(());
    }
    let session_id = self.maestro_session_id().await?;
    let line = compose_user_envelope(body);
    self.inner.supervisor.send_input(&session_id, line.into_bytes()).await
}
```

- [ ] **Step 4: Run, verify PASS** — `cargo test -p concerto-core maestro::handle`.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/handle.rs
git commit -m "feat(maestro): forward freeform as a stream-json user envelope"
```

---

## Task 4: `MaestroStreamJsonPack` — the parser

**Files:**
- Create: `crates/core/src/agent_supervisor/parsers/maestro_stream_json.rs`
- Modify: `crates/core/src/agent_supervisor/parsers/mod.rs` (`pub mod maestro_stream_json;`)
- Test: in-file `#[cfg(test)]` (table-driven over the Task-0 fixture)

Parse newline-delimited Claude `stream-json` into `ParseEvent`s. Buffer partial lines in `buf` (the trait supports this); only consume complete lines.

- [ ] **Step 1: Write the failing test** (feeds the fixture in two arbitrary byte splits to exercise partial-line buffering):

```rust
#[test]
fn parses_assistant_text_and_turn_complete_from_fixture() {
    let pack = MaestroStreamJsonPack::new();
    let data = include_bytes!("../../../tests/fixtures/maestro_stream_json/turn.jsonl");
    let mut buf = Vec::new();
    let mut events = Vec::new();
    // Feed in 7-byte chunks to prove partial lines are buffered, not dropped.
    for chunk in data.chunks(7) {
        buf.extend_from_slice(chunk);
        events.extend(pack.parse_chunk(&mut buf));
    }
    // Collect assistant message texts.
    let texts: Vec<String> = events.iter().filter_map(|e| match e {
        ParseEvent::Message { role: MsgRole::Assistant, content } => Some(content.clone()),
        _ => None,
    }).collect();
    assert!(texts.iter().any(|t| t.contains("check your workspaces")));
    assert!(texts.iter().any(|t| t.contains("1 workspace")));
    // No tool_use / tool_result leaked as a chat message.
    assert!(!texts.iter().any(|t| t.contains("tool_use") || t.contains("tool_result")));
    // Exactly one TurnComplete (the `result` line).
    assert_eq!(events.iter().filter(|e| matches!(e, ParseEvent::TurnComplete)).count(), 1);
}
```

- [ ] **Step 2: Run, verify FAIL** — pack undefined.

- [ ] **Step 3: Implement** the pack:

```rust
//! Maestro `stream-json` parser pack: adapts Claude's `--output-format
//! stream-json` event lines into `ParseEvent`s for the Maestro chat. Distinct
//! from the regex `ClaudeCodePack` (terminal scrape) — this is the structured
//! path the Maestro chat needs. Tool calls ride the MCP channel; here they are
//! swallowed (no chat bubble in M1).

use crate::agent_supervisor::parsers::{MsgRole, ParseEvent, ParserPack};
use crate::agent_supervisor::AgentKind;
use crate::security::Decision;

#[derive(Debug, Default, Clone)]
pub struct MaestroStreamJsonPack;

impl MaestroStreamJsonPack {
    pub fn new() -> Self { Self }
}

/// Extract one line's `ParseEvent`s from a parsed Claude stream-json object.
fn events_for(v: &serde_json::Value) -> Vec<ParseEvent> {
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            // assistant.message.content[] — emit only the text parts.
            let mut out = Vec::new();
            if let Some(parts) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for p in parts {
                    if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text) = p.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                out.push(ParseEvent::Message {
                                    role: MsgRole::Assistant,
                                    content: text.to_string(),
                                });
                            }
                        }
                    }
                }
            }
            out
        }
        Some("result") => {
            // Turn boundary. On error, surface the reason as an assistant
            // message so the user sees *why* (e.g. quota) instead of silence.
            let mut out = Vec::new();
            if v.get("is_error").and_then(|b| b.as_bool()) == Some(true) {
                let reason = v.get("result").and_then(|r| r.as_str()).unwrap_or("the Maestro hit an error");
                out.push(ParseEvent::Message { role: MsgRole::Assistant, content: reason.to_string() });
            }
            out.push(ParseEvent::TurnComplete);
            out
        }
        // system/init, user (tool results), unknown → no chat event.
        _ => Vec::new(),
    }
}

impl ParserPack for MaestroStreamJsonPack {
    fn agent_kind(&self) -> AgentKind { AgentKind::Maestro }
    fn version_pattern(&self) -> &str { r"(claude|codex|gemini)" }

    fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent> {
        let mut out = Vec::new();
        // Consume complete lines; retain the trailing partial line in `buf`.
        loop {
            let Some(nl) = buf.iter().position(|b| *b == b'\n') else { break };
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = &line[..line.len() - 1]; // strip '\n'
            let s = String::from_utf8_lossy(line);
            let s = s.trim();
            if s.is_empty() { continue; }
            match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) => out.extend(events_for(&v)),
                Err(e) => {
                    tracing::warn!(target: "concerto::maestro", error = %e, "skipping unparseable maestro stream-json line");
                }
            }
        }
        out
    }

    fn inject_approval(&self, _decision: Decision) -> Vec<u8> {
        // Tool gating is MCP + --allowedTools, never a PTY-scraped menu.
        Vec::new()
    }
}
```
Add `pub mod maestro_stream_json;` to `parsers/mod.rs`.

- [ ] **Step 4: Run, verify PASS** — `cargo test -p concerto-core agent_supervisor::parsers::maestro_stream_json` + `cargo clippy -p concerto-core --all-targets -- -D warnings`.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/agent_supervisor/parsers/maestro_stream_json.rs crates/core/src/agent_supervisor/parsers/mod.rs
git commit -m "feat(maestro): stream-json parser pack (assistant text + turn boundaries)"
```

---

## Task 5: Select the new pack for `AgentKind::Maestro`

**Files:**
- Modify: the pack-selection site (grep `MaestroPack::new()` — likely `crates/core/src/agent_supervisor/spawn.rs` or `actor.rs`)
- Test: a pack-selection unit test if one exists; otherwise `cargo build`

- [ ] **Step 1: Find + swap.** Grep `MaestroPack` for where the supervisor builds the parser pack per `AgentKind`. Replace the `AgentKind::Maestro => Box::new(MaestroPack::new())` arm with `Box::new(MaestroStreamJsonPack::new())`. Update the `use` import.

- [ ] **Step 2: Decide the fate of the old `MaestroPack`.** It is now unused. Remove `crates/core/src/agent_supervisor/parsers/maestro.rs` and its `pub mod maestro;` line **only if** nothing else references it (grep `MaestroPack` first). If a test references it, update that test to the new pack.

- [ ] **Step 3: Verify** — `cargo build -p concerto-core` + `cargo test -p concerto-core agent_supervisor` → green. `cargo clippy -p concerto-core --all-targets -- -D warnings` (catches the now-unused old pack if you left it).

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/agent_supervisor
git commit -m "feat(maestro): use the stream-json pack for the Maestro session"
```

**Executor note:** confirm the exact selection site and its match shape before editing (grep `MaestroPack::new`). If pack selection keys on something other than `AgentKind` (e.g. a provider hint), follow that real structure.

---

## Task 6: Events bridge — session turns → `maestro.events`

**Files:**
- Create: `crates/core/src/maestro/events_bridge.rs`
- Modify: `crates/core/src/maestro/mod.rs` (`pub mod events_bridge;`), `handle.rs::spawn_maestro_session` (spawn the bridge after `start_session`)
- Test: in-file `#[cfg(test)]`

A task that subscribes to the Maestro session's `AgentEvent`s, accumulates assistant text within a turn, and emits one `MaestroEvent::Message` per completed turn onto `maestro.events` (which `MaestroChat` renders). One bubble per turn (no delta streaming in M1).

- [ ] **Step 1: Write the failing test** (drive the accumulation logic directly, no live session):

```rust
#[test]
fn accumulator_emits_one_message_per_turn() {
    let mut acc = TurnAccumulator::default();
    assert!(acc.on_message("Let me check. ").is_none());
    assert!(acc.on_message("You have 1 workspace.").is_none());
    let done = acc.on_turn_complete().expect("a turn produces a message");
    assert_eq!(done.text, "Let me check. You have 1 workspace.");
    assert!(!done.message_id.is_empty());
    // A turn with no assistant text produces nothing.
    assert!(acc.on_turn_complete().is_none());
}
```

- [ ] **Step 2: Run, verify FAIL.**

- [ ] **Step 3: Implement** `events_bridge.rs`:

```rust
//! Bridges the Maestro session's parsed conversation onto the `maestro.events`
//! stream the chat UI renders. The supervisor publishes the Maestro session's
//! `AgentEvent::Message` (assistant text deltas) + `AgentEvent::TurnComplete`;
//! this accumulates a turn's text and emits one `MaestroEvent::Message` per
//! completed turn. (M1: one bubble per turn; delta streaming is a later polish.)

use std::sync::Arc;
use crate::agent_supervisor::events::AgentEvent;
use crate::agent_supervisor::AgentSupervisorHandle;
use crate::maestro::events::{MaestroEvent, MaestroEventSender};
use concerto_persist::SessionId;

/// A completed assistant turn ready to publish.
pub struct TurnMessage { pub text: String, pub message_id: String }

/// Accumulates assistant text within a turn. Pure + unit-testable.
#[derive(Default)]
pub struct TurnAccumulator { buf: String, seq: u64 }

impl TurnAccumulator {
    /// Append an assistant text delta. Returns `None` (M1 emits at turn end).
    pub fn on_message(&mut self, text: &str) -> Option<TurnMessage> {
        self.buf.push_str(text);
        None
    }
    /// Close the turn; emit the accumulated text (or `None` if empty).
    pub fn on_turn_complete(&mut self) -> Option<TurnMessage> {
        if self.buf.trim().is_empty() { self.buf.clear(); return None; }
        self.seq += 1;
        let msg = TurnMessage { text: std::mem::take(&mut self.buf), message_id: format!("m-{}", self.seq) };
        Some(msg)
    }
}

/// Spawn the bridge for the given Maestro session. Runs until the session's
/// event channel closes (session end / Core shutdown).
pub fn spawn_maestro_events_bridge(
    supervisor: AgentSupervisorHandle,
    events: MaestroEventSender,
    session_id: SessionId,
) {
    tokio::spawn(async move {
        let Some(mut rx) = supervisor.subscribe_events(&session_id).await else {
            tracing::warn!(target: "concerto::maestro", session = %session_id.0, "events bridge: no such session to subscribe");
            return;
        };
        let mut acc = TurnAccumulator::default();
        loop {
            match rx.recv().await {
                Ok(AgentEvent::Message { session_id: s, role, content }) if s == session_id => {
                    // Only assistant text becomes a bubble here; the user turn is
                    // published directly by send_to_maestro (Task 7).
                    if matches!(role, crate::agent_supervisor::events::MsgRole::Assistant) {
                        acc.on_message(&content);
                    }
                }
                Ok(AgentEvent::TurnComplete { session_id: s }) if s == session_id => {
                    if let Some(m) = acc.on_turn_complete() {
                        events.emit(MaestroEvent::Message { text: m.text, message_id: m.message_id });
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break, // channel closed
            }
        }
    });
}
```
Add `pub mod events_bridge;` to `maestro/mod.rs`. In `spawn_maestro_session`, after `start_session` returns the `SessionId`, spawn the bridge:
```rust
let sid = self.inner.supervisor.start_session(req).await?;
crate::maestro::events_bridge::spawn_maestro_events_bridge(
    self.inner.supervisor.clone(),
    self.inner.events.clone(),
    sid.clone(),
);
Ok(sid)
```

- [ ] **Step 4: Run, verify PASS** — `cargo test -p concerto-core maestro::events_bridge` + `cargo build -p concerto-core`.

**Executor notes:** confirm the real `AgentEvent` variant field names + the `MsgRole` path used on `AgentEvent::Message` (grep `enum AgentEvent` in `agent_supervisor/events.rs`; it maps from `MsgRole` via `map_msg_role` in the read-pump — match that role type). Confirm `MaestroEventSender` is `Clone` and `MaestroEvent::Message{text, message_id}` is the exact variant shape (it is, per events.rs).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/events_bridge.rs crates/core/src/maestro/mod.rs crates/core/src/maestro/handle.rs
git commit -m "feat(maestro): bridge session turns to maestro.events as chat messages"
```

---

## Task 7: Publish the user turn + persist it

**Files:**
- Modify: `crates/core/src/maestro/handle.rs` (`send_to_maestro` Freeform arm), `crates/persist/src/chat_messages.rs` (a user-turn insert if none fits)
- Test: in-file `#[cfg(test)]`

When a freeform message is accepted, echo it back as a `maestro.events` `message` (role=user) so the user's bubble appears immediately, and persist it to the maestro `chat_messages` (assistant turns already persist on `TurnComplete`).

- [ ] **Step 1: Write the failing test** — assert that `send_to_maestro` with a freeform body emits a `MaestroEvent::Message` for the user turn. Use a test `MaestroEventSender` with a subscriber (mirror the events.rs test that asserts `emit` reaches a receiver), build a handle around it, call the Freeform path, assert a user `message` event was emitted. (If wiring a full handle is heavy, factor the "publish + persist user turn" into a small `record_user_turn(&self, body)` helper and unit-test that helper directly.)

- [ ] **Step 2: Run, verify FAIL.**

- [ ] **Step 3: Implement.** In the `ParseOutcome::Freeform(body)` arm of `send_to_maestro`, before `forward_freeform`:
```rust
ParseOutcome::Freeform(body) => {
    self.guard_llm().await?;
    self.record_user_turn(&body).await; // best-effort: publish + persist
    self.forward_freeform(&body).await?;
    Ok(SendOutcome::Forwarded)
}
```
Add `record_user_turn`:
```rust
/// Publish the user's turn to `maestro.events` (immediate bubble) and persist
/// it to the maestro chat. Best-effort: a persistence hiccup must not block the
/// forward.
async fn record_user_turn(&self, body: &str) {
    self.inner.events.emit(MaestroEvent::Message {
        text: body.to_string(),
        message_id: String::new(), // user turns aren't delta-grouped
    });
    if let Ok(chat_id) = self.ensure_maestro_chat().await {
        if let Err(e) = concerto_persist::chat_messages::insert_user_message(
            &self.inner.persistence, &chat_id, body,
        ).await {
            tracing::warn!(target: "concerto::maestro", error = %e, "failed to persist maestro user turn");
        }
    }
}
```
**Executor note:** `MaestroEvent::Message` carries only `{text, message_id}` (no role) — the frontend infers role from context, OR you add an optional `role` to the variant + its JSON (additive; update `events.rs` serialization + the frontend `MaestroEvent` type). Decide: simplest is to add `role` to `MaestroEvent::Message` so user vs assistant bubbles render correctly. If you add `role`, thread it through the Task-6 emit too (`role:"assistant"`). Add `chat_messages::insert_user_message(persist, chat_id, text)` mirroring how `checkpoint::insert_turn_message` writes an assistant row (role='user', content_json = the text).

- [ ] **Step 4: Run, verify PASS** + `cargo build -p concerto-core`.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/handle.rs crates/persist/src/chat_messages.rs crates/core/src/maestro/events.rs
git commit -m "feat(maestro): echo + persist the user turn on freeform send"
```

---

## Task 8: History on open — `GetHistory` RPC + desktop load

**Files:**
- Modify: `crates/persist/src/chat_messages.rs` (`list_by_chat`), `crates/proto/.../maestro.proto` (`GetHistory`), `crates/core/src/handlers/maestro.rs`, `crates/core/src/maestro/handle.rs` (`get_history`), `apps/desktop/src/api/maestro.ts`, `apps/desktop/src/components/maestro/MaestroChat.tsx`, `apps/desktop/src-tauri/src/rpc.rs`
- Test: persist test for `list_by_chat`; handler/desktop tests as they exist

So the conversation + digest survive a reload, load the maestro chat history when `MaestroChat` mounts.

- [ ] **Step 1: persist `list_by_chat`** — add `pub async fn list_by_chat(pool: &SqlitePool, chat_id: &str, limit: i64) -> Result<Vec<ChatMessage>>` returning the most-recent `limit` messages for `chat_id` ordered by `created_at` ASC. Write a persist test (insert 2, assert order + content). Mirror the SQL style of `list_in_day_range`.

- [ ] **Step 2: proto + handler** — add `rpc GetHistory(GetHistoryRequest) returns (MaestroHistory);` where `MaestroHistory { repeated MaestroTurn turns = 1; }` and `MaestroTurn { string role = 1; string text = 2; int64 created_at_ms = 3; }`. Regenerate (`cargo build -p concerto-proto`). Add `MaestroHandle::get_history()` → `ensure_maestro_chat()` + `chat_messages::list_by_chat` → map rows to turns (decode `content_json` to text). Add the handler arm mapping to the proto.

- [ ] **Step 3: desktop** — add `getHistory()` in `api/maestro.ts` (calls `Maestro.GetHistory`), add the `"Maestro.GetHistory"` arm to `src-tauri/src/rpc.rs` (mirror the `GetDigest` arm), and in `MaestroChat.tsx` call `getHistory()` on mount to seed the transcript before live events arrive (dedupe live events that duplicate the last persisted turn by `message_id`/text if needed).

- [ ] **Step 4: Verify** — `cargo test -p concerto-persist chat_messages`, `cargo build -p concerto-proto -p concerto-core`, `cargo check -p concerto-desktop --no-default-features`, desktop `pnpm --filter? typecheck` (use the command from Task 9 of the prior plan: `cd apps/desktop && pnpm run typecheck && pnpm run test`).

- [ ] **Step 5: Commit**

```bash
git add crates/persist/src/chat_messages.rs crates/proto crates/core/src/handlers/maestro.rs crates/core/src/maestro/handle.rs apps/desktop
git commit -m "feat(maestro): persist + load chat history (survives reload)"
```

---

## Task 9: Integration test + full gate

**Files:**
- Create: `crates/core/tests/maestro_conversation.rs`

Drive the full loop with a scripted fake `stream-json` agent (no real Claude): a fake agent bin that, on receiving a `stream-json` user envelope on stdin, emits the Task-0 fixture's assistant + result lines on stdout. Assert a `maestro.events` `message` (role=assistant) carrying the reply reaches the stream.

- [ ] **Step 1: Write the test.** Reuse the supervisor's fake-agent harness (the Echo-style bin used by `tests/agent_spawn.rs`) OR a tiny scripted bin. Boot the maestro spine with the fake agent as the resolved CLI; subscribe to `maestro.events`; call `send_to_maestro("hi", None)`; assert (a) a user `message` event, then (b) an assistant `message` event containing the fixture reply text. If driving the real spawn path with a fake `AgentKind::Maestro` bin is too heavy, narrow to: feed the fixture bytes through `MaestroStreamJsonPack` → `TurnAccumulator` → assert a `MaestroEvent::Message` is produced (the seam wiring), and rely on Task 6's unit test + Tier-3 for the live path.

- [ ] **Step 2: Run, verify PASS.**

- [ ] **Step 3: Full gate** — `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/`, `cd apps/desktop && pnpm run typecheck && pnpm run test`. Fix any drift.

- [ ] **Step 4: Commit**

```bash
git add crates/core/tests/maestro_conversation.rs
git commit -m "test(maestro): end-to-end conversation loop (fake stream-json agent → maestro.events)"
```

---

## Manual verification (Tier-3 — needs real Claude; mind the quota)

1. `cargo run --bin concerto-core` (or the built binary) from this branch; log shows `maestro session live`.
2. `pnpm tauri dev`; open the app.
3. Maestro composer: `what are my workareas doing?` → your bubble appears, then a **streamed/whole grounded reply** that used the read tools (no "nothing happens", no trust prompt in the session log).
4. `@<composer> <msg>` in the viewed workspace → routes (already worked in #178).
5. Reload the app (Cmd+R) → the conversation + digest persist.
6. Confirm the Maestro session's `stdout.log` shows JSON event lines (not a TUI), proving headless mode.

---

## Self-Review

**Spec coverage:** provider stream-json (Task 1) ✓ · trust preseed/#2 (Task 2) ✓ · input framing (Task 3) ✓ · parser (Task 4) ✓ · pack selection (Task 5) ✓ · events bridge/#5 (Task 6) ✓ · user-turn echo+persist (Task 7) ✓ · history on open/#6 (Task 8) ✓ · integration + gate (Task 9) ✓ · spike/fixture (Task 0) ✓. Error handling: unparseable-line skip (Task 4), quota→assistant message (Task 4 `result.is_error`), best-effort persistence (Task 7). All spec sections map.

**Placeholder scan:** code steps carry real code. Three explicit executor notes (Task 5 selection site, Task 6 `AgentEvent` field/role names, Task 7 optional `role` on `MaestroEvent::Message`) are genuine "match the local shape" points — not hidden decisions. The Task-7 `role` decision is made explicit (add `role` to `MaestroEvent::Message`).

**Type consistency:** `compose_user_envelope` (T3) used by `forward_freeform` (T3). `MaestroStreamJsonPack` (T4) selected in T5, fed in T9. `TurnAccumulator`/`spawn_maestro_events_bridge` (T6) called in `spawn_maestro_session` (T6). `MaestroEvent::Message{text, message_id (+role)}` emitted in T6/T7, consumed by `MaestroChat`. `ensure_maestro_mcp_trusted` (T2) called in `spawn_maestro_session` (T2). `chat_messages::insert_user_message` (T7) + `list_by_chat` (T8) consistent. `get_history`/`GetHistory` (T8) consumed by the desktop (T8).

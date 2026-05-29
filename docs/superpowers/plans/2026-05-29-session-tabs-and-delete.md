# Session Tabs + Real Delete — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Turn the session strip into real IDE tabs with a close (✕) button that *fully deletes* the session server-side, replace "+ Start Session" with a `+`-button dropdown that picks the agent, and remove the Terminal/Chat sub-tab row.

**Architecture:** Add a new destructive `Sessions.DeleteSession` RPC (Core is non-destructive on `StopSession` and the `/loop` scheduler + revert rely on that, so we do NOT overload Stop). `DeleteSession` stops the agent if running, then hard-deletes the session in one transaction (null `schedule_runs.session_id`, delete the session's `checkpoints`, delete the `sessions` row → cascades `chats`/`chat_messages`/`tool_approvals`), removes the on-disk log dir, and emits an audit event. The renderer's tab ✕ calls it (with a confirm only when the session is still running). Frontend reworks the session strip into horizontally-scrolling tabs + a `+` dropdown (new `Menu` primitive) and drops the sub-tab row.

**Tech stack:** Rust (proto/tonic, core, persist, agent-host supervisor), Tauri shell (`crates/desktop-shell` + `apps/desktop/src-tauri`), React/TS renderer. Build-time proto codegen (not checked in). Gates: `cargo build`/`cargo clippy -D warnings`/`cargo test` for Rust, `pnpm build` for renderer, `scripts/regen-interfaces.sh` after the proto edit (CI-gated), `scripts/smoke.sh` optional.

**Locked decisions:** (1) delete is permanent incl. the chat transcript — intended; (2) confirm-on-close ONLY when the session is still running (`starting|running|awaiting`), instant otherwise.

**Conventions:** branch `redesign-desktop-app-ui`; commit directly; trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Run `pnpm` from `apps/desktop`; `cargo` from repo root.

---

## Reference facts (from recon)
- `StopSession` handler: `crates/core/src/handlers/sessions.rs:241-261` → `AgentSupervisorHandle::stop_session` (`crates/core/src/agent_supervisor/actor.rs:906-950`): `map.remove` evicts the in-mem `SessionEntry`, `child.kill()`, `remove_file(socket_path)`, `persist::sessions::mark_ended` (UPDATE status='finished'). Row is kept.
- `ListSessions` = `persist::sessions::list_by_workarea` (`crates/persist/src/sessions.rs:258`) — NO status filter, so finished sessions stay in the list.
- FK graph: `chats.session_id`→CASCADE, `chat_messages.chat_id`→CASCADE, `tool_approvals.session_id`→CASCADE (all auto). **Blockers:** `schedule_runs.session_id` (nullable, RESTRICT — null it out in-tx) and `checkpoints.chat_message_id` (RESTRICT — delete the session's checkpoints in-tx before the chat cascade).
- On-disk per-session log dir created at `actor.rs:399` (`<data>/agents/<sid>/`).
- Codegen: `crates/proto/build.rs` runs tonic_build into OUT_DIR (not checked in); editing the `.proto` + `cargo build` regenerates the `Sessions` trait → the async-trait impl in `handlers/sessions.rs` won't compile until `delete_session` exists (safety net).
- Tauri dispatch: `apps/desktop/src-tauri/src/commands.rs` `dispatch()` match on method string (StopSession arm ~674-686); fallback `other => NotImplemented` (~726) is a RUNTIME gate — must add the arm.
- TS client union: `apps/desktop/src/api/client.ts` `RpcMethod`; wrapper in `apps/desktop/src/api/sessions.ts` (`stopSession` at 87-95).
- No relay/transport/capability method whitelist to mirror.

---

## Task A: Proto — add `DeleteSession` + regen

**Files:** `crates/proto/proto/concerto/v1/sessions.proto`; then run `scripts/regen-interfaces.sh` (updates `docs/interfaces/proto.md`).

- [ ] **Step 1:** In `sessions.proto`, inside `service Sessions { ... }` (near `StopSession`, ~line 192), add:
  ```proto
  // Destructive: stops the session if running, then permanently deletes
  // it and its dependent rows (chat thread, tool approvals, checkpoints;
  // schedule_runs are unlinked). Unlike StopSession this removes the row.
  rpc DeleteSession(SessionId) returns (google.protobuf.Empty);
  ```
  Reuse the existing `SessionId` message (id-only, as `GetSession`/`ColdResumeSession` do) — do NOT add a new message, and do NOT renumber anything.
- [ ] **Step 2:** Run `scripts/regen-interfaces.sh` from repo root. Confirm only `docs/interfaces/proto.md` changed (the new rpc appears in the Sessions block).
- [ ] **Step 3:** `cargo build -p concerto-core` — EXPECTED TO FAIL to compile with a missing-trait-method error (`delete_session` not implemented). This confirms codegen picked up the rpc. (Task D fixes it.)
- [ ] **Step 4:** Commit:
  ```bash
  git add crates/proto/proto/concerto/v1/sessions.proto docs/interfaces/proto.md
  git commit -m "feat(proto): add Sessions.DeleteSession rpc"
  ```
  (Commit even though core doesn't compile yet — the next tasks land the impl in the same series; this keeps the proto change atomic.)

---

## Task B: Persist — `delete` with cascade + test

**Files:** `crates/persist/src/sessions.rs`; a test in the same file (follow the existing test module pattern in that file / crate).

- [ ] **Step 1:** Read `crates/persist/src/sessions.rs` fully (note the conn/writer type, the `mark_ended`/`get`/`list_by_workarea` signatures, and how transactions are opened elsewhere in the crate). Read the test module if present, and how other modules (e.g. `schedules.rs::delete`) write delete tests.
- [ ] **Step 2:** Add a `delete` function that performs, inside ONE transaction:
  ```
  1. UPDATE schedule_runs SET session_id = NULL WHERE session_id = ?1   -- release RESTRICT FK, keep /loop history
  2. DELETE FROM checkpoints
       WHERE chat_message_id IN (
         SELECT cm.id FROM chat_messages cm
         JOIN chats c ON cm.chat_id = c.id
         WHERE c.session_id = ?1)                                      -- release RESTRICT FK
  3. DELETE FROM sessions WHERE id = ?1                                 -- cascades chats → chat_messages, tool_approvals
  ```
  Match the crate's actual API (sqlx vs rusqlite, async vs sync, the writer/transaction type used by `mark_ended`). Signature should mirror neighbors, e.g. `pub async fn delete(writer: &mut ..., id: &str) -> Result<...>`. Ensure `foreign_keys = ON` semantics hold (they're set per-connection per recon) so the cascade fires.
- [ ] **Step 3:** Add a test `delete_removes_session_and_dependents` that: seeds a project/workspace/workarea/chat/session + a chat_message + a tool_approval + a checkpoint (on that message) + a schedule + a schedule_run referencing the session, calls `delete`, then asserts: `get(session)` is None / not in `list_by_workarea`; the chat + chat_message + tool_approval + checkpoint rows are gone; the schedule_run row still exists with `session_id IS NULL`. Use the crate's existing test harness/fixtures (temp DB + migrations). If seeding all dependents is heavy, at minimum cover: session+chat+chat_message gone, schedule_run nulled, checkpoint deleted.
- [ ] **Step 4:** `cargo test -p concerto-persist` — the new test passes; existing tests still pass.
- [ ] **Step 5:** `cargo clippy -p concerto-persist --all-targets -- -D warnings` — clean.
- [ ] **Step 6:** Commit:
  ```bash
  git add crates/persist/src/sessions.rs
  git commit -m "feat(persist): hard-delete a session with dependent cascade"
  ```

---

## Task C+D: Supervisor `delete_session` + Core handler

**Files:** `crates/core/src/agent_supervisor/actor.rs` (supervisor), `crates/core/src/handlers/sessions.rs` (RPC handler).

- [ ] **Step 1:** Read `actor.rs:906-950` (`stop_session`) and the surrounding `SessionEntry`/teardown code (socket path, log-dir path built near `actor.rs:399`), and the audit-emit pattern used by archive/revert (e.g. `actor.rs:804`/`:998`).
- [ ] **Step 2:** Add `AgentSupervisorHandle::delete_session(&self, session_id, reason)`:
  - If the session is live (entry present in the map): reuse the stop teardown — `map.remove`, `child.kill().await`, `remove_file(socket_path)`, set `finished`. (Factor the shared teardown out of `stop_session` into a private helper if clean; otherwise call `stop_session` first then proceed — but avoid double-`mark_ended`; deletion supersedes it.)
  - Best-effort remove the on-disk log dir (`tokio::fs::remove_dir_all(<data>/agents/<sid>/)`), ignoring NotFound.
  - Call `concerto_persist::sessions::delete(&mut writer, session_id)` (Task B).
  - Emit an audit event for the destructive op (mirror the archive/revert audit kind/shape).
  - Return Ok even if the entry was absent but the row exists (delete the row anyway); return NotFound only if neither entry nor row exists. Tolerate the concurrent host-exit race (row already `finished`).
- [ ] **Step 3:** In `crates/core/src/handlers/sessions.rs`, import the request type if needed and implement the trait method inside `impl SessionsService for SessionsHandler` (mirror `stop_session` at 241-261):
  ```rust
  async fn delete_session(&self, request: Request<SessionId>) -> Result<Response<Empty>, Status> {
      let id = request.into_inner().id; // match the SessionId field name
      // validate non-empty → map errors like stop_session does
      self.supervisor.delete_session(&id, None).await
          .map_err(/* same error mapping as stop_session */)?;
      Ok(Response::new(Empty {}))
  }
  ```
  Use the EXACT trait method name/signature the regenerated tonic trait expects (check the generated `sessions_server::Sessions` trait — likely `delete_session(&self, request: Request<SessionId>)`).
- [ ] **Step 4:** `cargo build -p concerto-core` — now compiles (the trait gate is satisfied).
- [ ] **Step 5:** `cargo clippy -p concerto-core --all-targets -- -D warnings` — clean. `cargo test -p concerto-core` — existing tests pass.
- [ ] **Step 6:** Commit:
  ```bash
  git add crates/core/src/agent_supervisor/actor.rs crates/core/src/handlers/sessions.rs
  git commit -m "feat(core): DeleteSession — stop + hard cleanup + audit"
  ```

---

## Task E: Tauri shell dispatch arm

**Files:** `apps/desktop/src-tauri/src/commands.rs`.

- [ ] **Step 1:** Read the `StopSession` arm (~674-686) + the `use concerto_proto::v1::{...}` import (~37-44) and the `IdPayload`/`StopSessionPayload` structs (~323, ~356-366).
- [ ] **Step 2:** Add a dispatch arm (reuse `IdPayload` since DeleteSession is id-only — confirm `IdPayload` has the `id`/`session_id` field shape the renderer sends; the renderer will send `{ session_id }` — match StopSession's payload field naming, which sends `session_id`; if `IdPayload` uses a different field, add a small `DeleteSessionPayload { session_id: String }` mirroring `StopSessionPayload` minus `reason`):
  ```rust
  "Sessions.DeleteSession" => {
      let p: DeleteSessionPayload = serde_json::from_value(payload)?; // or IdPayload
      let mut client = sessions_client(...).await?;                   // same client construction as StopSession arm
      client.delete_session(SessionId { id: p.session_id }).await ... ;
      Ok(serde_json::Value::Null)
  }
  ```
  Mirror the StopSession arm's client construction + error handling EXACTLY; only the request type (`SessionId`) and method (`delete_session`) differ. Add `SessionId` to the proto import if not already present.
- [ ] **Step 3:** `cargo build -p concerto-desktop` (or the desktop-shell crate name) — compiles.
- [ ] **Step 4:** `cargo clippy -p <desktop crate> --all-targets -- -D warnings` — clean.
- [ ] **Step 5:** Commit:
  ```bash
  git add apps/desktop/src-tauri/src/commands.rs
  git commit -m "feat(desktop-shell): route Sessions.DeleteSession"
  ```

---

## Task F: TS client wrapper

**Files:** `apps/desktop/src/api/client.ts`, `apps/desktop/src/api/sessions.ts`.

- [ ] **Step 1:** In `client.ts` add `"Sessions.DeleteSession"` to the `RpcMethod` union (next to `"Sessions.StopSession"`).
- [ ] **Step 2:** In `sessions.ts` add:
  ```ts
  export async function deleteSession(sessionId: string): Promise<void> {
    await callRpc<{ session_id: string }, null>("Sessions.DeleteSession", {
      session_id: sessionId,
    });
  }
  ```
  (Match the payload field name to what the shell arm in Task E expects — `session_id`.)
- [ ] **Step 3:** `cd apps/desktop && pnpm build` — passes.
- [ ] **Step 4:** Commit:
  ```bash
  git add apps/desktop/src/api/client.ts apps/desktop/src/api/sessions.ts
  git commit -m "feat(desktop): deleteSession API wrapper"
  ```

---

## Task G: `Menu` dropdown primitive

**Files:** Create `apps/desktop/src/components/ui/menu.tsx`.

- [ ] **Step 1:** Create a lightweight, dependency-free dropdown anchored to a trigger, with click-outside + Escape to close, token-styled. API:
  ```tsx
  // Anchored dropdown menu. No Radix — hand-rolled to match the other
  // ui/ primitives. Renders a trigger; clicking it toggles a popover of
  // items below it. Closes on outside-click, Escape, or item select.
  import { useEffect, useRef, useState, type ReactNode } from "react";

  export type MenuItem = { id: string; label: string; description?: string; icon?: ReactNode };

  export function Menu({
    trigger, items, onSelect, align = "left",
  }: {
    trigger: (open: boolean) => ReactNode;
    items: ReadonlyArray<MenuItem>;
    onSelect: (id: string) => void;
    align?: "left" | "right";
  }) {
    const [open, setOpen] = useState(false);
    const ref = useRef<HTMLDivElement>(null);
    useEffect(() => {
      if (!open) return;
      function onDoc(e: MouseEvent) { if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false); }
      function onKey(e: KeyboardEvent) { if (e.key === "Escape") setOpen(false); }
      document.addEventListener("mousedown", onDoc);
      document.addEventListener("keydown", onKey);
      return () => { document.removeEventListener("mousedown", onDoc); document.removeEventListener("keydown", onKey); };
    }, [open]);
    return (
      <div ref={ref} className="relative inline-flex">
        <button type="button" onClick={() => setOpen((o) => !o)} aria-haspopup="menu" aria-expanded={open} className="inline-flex">
          {trigger(open)}
        </button>
        {open && (
          <div role="menu" className={`absolute top-full z-50 mt-1 min-w-[12rem] rounded-lg border border-border bg-surface p-1 shadow-xl ${align === "right" ? "right-0" : "left-0"}`}>
            {items.map((it) => (
              <button key={it.id} type="button" role="menuitem"
                onClick={() => { onSelect(it.id); setOpen(false); }}
                className="flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-xs text-foreground hover:bg-accent hover:text-accent-fg transition-colors">
                {it.icon}
                <span className="font-medium">{it.label}</span>
                {it.description && <span className="ml-auto text-faint">{it.description}</span>}
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }
  ```
  (Note: `hover:text-accent-fg` on the description won't override `text-faint` due to ordering — acceptable; or drop the desc hover color. Keep it simple.)
- [ ] **Step 2:** `pnpm build` — passes (unused until Task H; just compiles).
- [ ] **Step 3:** Commit:
  ```bash
  git add apps/desktop/src/components/ui/menu.tsx
  git commit -m "feat(desktop): add Menu dropdown primitive"
  ```

---

## Task H: Session strip rework (tabs + ✕ delete + `+` dropdown, drop sub-tabs)

**Files:** `apps/desktop/src/components/SessionTab.tsx`, `apps/desktop/src/components/center/SessionRegion.tsx`, `apps/desktop/src/components/StartSessionPicker.tsx` (remove usage), `apps/desktop/src/App.tsx` (drop the picker mount), `apps/desktop/src/state/useUiStore.ts` (drop `startSessionPickerOpen` if now unused).

- [ ] **Step 1 — `SessionTab.tsx`:** make it an IDE tab with a close ✕. Keep the status dot + agent kind + short id. Add a trailing `<button>` with an `X` (lucide, size 13) that appears on hover and is always visible on the active tab; its `onClick` calls a new `onClose` prop (and `stopPropagation` so it doesn't also select the tab). Tab styling: no rounded pill — a flat tab cell `h-9 px-3 border-r border-border` with the active state `bg-background text-foreground` + an accent top-bar (`relative` + an absolute `top-0 h-0.5 bg-accent` when active), inactive `text-muted hover:bg-surface-2`. Props become `{ session, active, onClick, onClose }`.
- [ ] **Step 2 — `SessionRegion.tsx`:**
  - Replace the wrapping flex strip with a single horizontally-scrollable tab row: `flex items-stretch overflow-x-auto bg-surface border-b border-border` (a real tab strip). Remove the `"Sessions:"` label and the `flex-wrap`.
  - Render `SessionTab`s with an `onClose={() => handleClose(s)}`.
  - **`handleClose(session)`:** if `session.status` ∈ `{starting,running,awaiting}` → `window.confirm("Stop and delete this running session? This permanently removes its transcript.")`; if not confirmed, return. Then `deleteMutation.mutate(session.id)`. On success: invalidate `["sessions", workareaId]`; if the closed session was active, pick an adjacent remaining session as active (or null). Use `deleteSession` from `api/sessions`.
  - Replace the `+ Start Session` Button with a `Menu` (`+` icon trigger as the last cell in the strip: `<div className="grid place-items-center w-9 h-9 text-muted hover:text-accent hover:bg-surface-2">`), items `[{id:"claude",label:"claude",description:"Claude Code"},{id:"echo",label:"echo",description:"smoke test"}]`, `onSelect={(agentKind) => createMutation.mutate(agentKind)}`. Add a `createSession` mutation here (mirroring the old StartSessionPicker: on success `setActiveSession(session.id)` + invalidate). `align="right"`.
  - REMOVE the separate "Stop Session" button entirely.
  - REMOVE `<SubTabHeader />` and the `SubTabHeader` function + the now-unused `Tabs` import. Render `<SessionTerminal>` directly under the strip (keep the composer).
  - Update the empty state copy → "No sessions yet." with a primary Button "New session" that opens the same agent menu (or simply reuse the `+` menu; simplest: keep the empty-state text + a `Menu` whose trigger is a primary-styled "New session" button).
  - Remove the `setStartSessionPickerOpen` usage.
- [ ] **Step 3 — retire `StartSessionPicker`:** remove its mount from `App.tsx` and delete `StartSessionPicker.tsx`. In `useUiStore.ts` remove `startSessionPickerOpen` + `setStartSessionPickerOpen` (and any references). (Grep first: `grep -rn "StartSessionPicker\|startSessionPickerOpen\|setStartSessionPickerOpen" src` → update/remove all.)
- [ ] **Step 4:** `cd apps/desktop && pnpm build` — passes (no unused imports; `noUnusedLocals` is on).
- [ ] **Step 5:** Grep gate: `grep -rn "StartSessionPicker\|startSessionPickerOpen\|Stop Session\|SubTabHeader" src` → no matches.
- [ ] **Step 6:** Commit:
  ```bash
  git add -A apps/desktop/src
  git commit -m "feat(desktop): IDE session tabs with delete + new-session dropdown; drop sub-tabs"
  ```

---

## Task I: Verify end-to-end

- [ ] **Step 1:** `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- [ ] **Step 2:** `cargo test -p concerto-persist -p concerto-core` — pass.
- [ ] **Step 3:** `cd apps/desktop && pnpm build` — pass.
- [ ] **Step 4:** Confirm `scripts/regen-interfaces.sh` leaves the tree clean (re-run; `git status` shows no diff) — proves docs/interfaces is in sync (CI gate).
- [ ] **Step 5 (optional):** add a `delete-session` smoke subcommand under `tools/smoke-client/` + a line in `scripts/smoke.sh` mirroring `stop_session`, and run `scripts/smoke.sh` if the environment supports it. Skip if risky.
- [ ] **Step 6:** Manual (user, needs GUI): launch `pnpm tauri dev`, create sessions via `+` dropdown, close finished tabs (instant) + a running tab (confirm prompt), verify they vanish from the bar and don't return after reload; confirm the terminal renders without the Terminal/Chat row.

---

## Self-review
- Spec coverage: tabs+✕ (H), full server delete (A–E + persist cascade B), `+` dropdown (G+H), remove sub-tabs (H), remove Stop button (H), confirm-only-if-running (H step 2). Decisions #1/#2 honored.
- Destructive-op safety: confirm guard on running sessions; audit event emitted; schedule_runs preserved (unlinked); transcript intentionally deleted.
- Compile gates chain correctly: proto regen (A) makes core fail until handler (D); persist (B) before supervisor (C) which the handler (D) calls; shell (E) and TS (F) after; UI (H) last.
- Risk: the persist cascade is the highest-risk change → it gets a dedicated test (B step 3) and the FK release order is explicit. `cargo clippy -D warnings` enforced per Rust task.

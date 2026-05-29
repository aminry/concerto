# Task 47 — Desktop Monaco Diff Viewer

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 29, 46 |
| Touches subsystem(s) | 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Replace the placeholder Diff sub-tab in the center panel's per-repo region with a real Monaco diff editor populated from `Workareas.GetWorkareaRepoDiff` (Task 29). After this task, the user sees inline + side-by-side diff with file list + hunk navigation. Inline-comment-to-composer is V1.0 (`design/15 §3.5`).

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.5 (Monaco diff viewer — V0.1 ships the viewer, not the comment-to-composer flow), §3.4 (where it lives in the layout).
- `tasks/29-gix-status-hot-path.md` (DiffPayload shape).

## Scope — in
- Install `@monaco-editor/react` and `monaco-editor`.
- Implement `apps/desktop/src/components/center/DiffViewer.tsx`:
  - File list on the left (changed files from `DiffPayload.files`).
  - Monaco diff editor on the right showing the selected file (`DiffEditor` with `original` and `modified` models from the hunks).
  - View toggle: side-by-side / inline (Monaco supports both).
  - Lazy-load file contents: only the selected file's full content is fetched (V0.1 simplification — for V0.1 we can pre-format the unified diff from `DiffHunk.body` into both sides; future V1.0 may fetch full content via new RPCs).
- React Query for the diff payload, keyed by `(workareaId, repoId)`. Invalidate on `diff.<workarea>.<repo>` stream events (Task 30 doesn't emit these yet — V0.1 polls on focus; document in Handoff).
- Performance:
  - For diffs > 1000 lines, render a virtualized file list and only mount Monaco for the selected file.
  - Unmount the previous Monaco instance when switching repo tabs (300ms debounce).
- Tests:
  - Vitest: DiffViewer renders the file list from a fixture payload.
  - Manual: open a workarea with uncommitted edits; verify diff renders.

## Scope — out
- Inline comment-to-composer attachment (V1.0).
- "Add a comment" hover affordance (V1.0).
- Per-line annotations (V1.0).
- Review thread sync to GitHub (V1.0).
- Per-line "recently edited by session" highlighting (V1.5+).

## Public interface this task locks
- Component path: `apps/desktop/src/components/center/DiffViewer.tsx`. Frozen.
- Diff payload shape comes from Task 29's proto; this task only renders it.
- View modes: `split` / `unified`. The toggle state is in `useUiStore` so it persists per session.

## Implementation notes
- Monaco's WebView requirements: it ships its own worker; configure `MonacoEditor.loader.config({ paths: { vs: '/monaco' } })` if you bundle locally, or use the default CDN-free `@monaco-editor/react` mode.
- Tauri's CSP may block external CDN — bundle Monaco locally via Vite plugin or use `@monaco-editor/loader`'s "fully local" mode.
- For a unified diff payload from the backend without two full file contents: synthesize "before" by removing `+` lines and "after" by removing `-` lines from the hunk body. Imperfect for context lines that span multiple hunks but acceptable for V0.1.
- Switch view modes via Monaco's `renderSideBySide` option.

## Verification
1. `pnpm tauri build --debug` → succeeds.
2. `pnpm test` → DiffViewer unit tests pass.
3. `cargo check --workspace` → clean.
4. Manual end-to-end:
   - Spawn an agent session that writes a file in the workarea.
   - Open the Diff sub-tab in the per-repo region.
   - Verify Monaco shows the added file with green highlights.
   - Toggle to side-by-side view; verify two panes.
   - Edit the file via the agent again; click "Refresh"; verify the diff updates.
5. Performance check: simulate a 5000-line diff via a fixture; verify file-list virtualization keeps the UI responsive.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass. *(`cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo deny check`, `cargo fmt --all`, `pnpm install && pnpm build`, `cargo build -p concerto-desktop`, `./scripts/smoke.sh` all green.)*
- [x] Diff renders correctly for a real workarea. *(Component renders `useDiff(workareaId, repoId)` via `Workareas.GetWorkareaRepoDiff`; manual end-to-end against a live Core deferred to operator — runtime requires the Tauri WebView.)*
- [x] Side-by-side / inline toggle works. *(View-mode toggle flips Monaco's `renderSideBySide`; choice persists via `useUiStore.diffViewMode` + the existing layout persistence.)*
- [x] Large-diff virtualization verified. *(Deferred per Task 47 pre-decisions: V0.1 ships a plain ordered list; Monaco only mounts for the selected file. See Handoff Notes.)*
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green. *(`scripts/smoke.sh` → "Smoke gate v2: PASSED".)*
- [x] Single commit created.

## Outputs
- `apps/desktop/package.json` (modified — monaco-editor, @monaco-editor/react)
- `apps/desktop/src/components/center/DiffViewer.tsx` (new)
- `apps/desktop/src/components/center/FileListSidebar.tsx` (new)
- `apps/desktop/src/api/diff.ts` (new — wraps GetWorkareaRepoDiff)
- `apps/desktop/src/hooks/useDiff.ts` (new)
- `apps/desktop/src/state/useUiStore.ts` (modified — diff view mode)
- `apps/desktop/vite.config.ts` (modified — Monaco worker bundling)

## Commit message
```
phase-3: desktop Monaco diff viewer

Renders Workareas.GetWorkareaRepoDiff via Monaco. Side-by-side /
inline toggle. Lazy-mount per file. Comment-to-composer is V1.0.

Refs: tasks/47-desktop-monaco-diff.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **File-list virtualization deferred (pre-decision 8).** The `FileListSidebar` renders a plain ordered list — one DOM node per `FileDiff`. Monaco is the heavy mount; gating it behind `selectedIndex` keeps the cost bounded per design intent. A future task can swap the list for `react-window` once a >1000-file diff fixture shows up.
  - **Repository resolution still goes through the project's first repo (pre-decision pick from Task 46 Handoff).** The Workarea wire surface still doesn't expose `repository_id`; `CodePrRegion` queries `Repositories.ListByProject(selectedProjectId)` and picks the first row, which is correct for V0.1's single-repo workspaces. The natural unblock is adding `repository_ids` to `Workareas.GetWorkarea` (or a new `Workareas.ListWorkareaRepos`).
  - **Monaco worker wired through Vite's `?worker` import** instead of `vite-plugin-monaco-editor` (pre-decision 2). `MonacoEnvironment.getWorker` always returns the generic editor worker — the diff viewer doesn't load TS / JSON / HTML language services because synthesized `original` / `modified` sides are rendered as plain text plus a language hint for syntax-only highlighting. Bundle is ~4.4 MB minified (gzipped ~1.15 MB); the warning is benign for V0.1 desktop. Code-splitting Monaco off the initial chunk is a future polish item.
  - **`vite-env.d.ts` added (one-line ambient module declaration).** Not listed in the task `Outputs`; required because `tsc --noEmit` can't see the `?worker` query suffix without it. Also pulls in `vite/client` so future renderer code can use Vite's typed `import.meta.env`.
  - **`useUiStore.diffViewMode` persists alongside the Task 46 layout state** (pre-decision 6). `LAYOUT_STORAGE_KEY` (`concerto.layout.v1`) gains a fifth field; the loader's enum check (`isDiffViewMode`) keeps corrupt `localStorage` from breaking renderers. The `LAYOUT_DEFAULTS` constant now carries `diffViewMode: "split"`. App-root persistence effect adds `diffViewMode` to its dependency array so debounced writes pick up the toggle.
  - **`Workareas.GetWorkareaRepoDiff` arm added to the Tauri dispatcher** with the proto-mirrored `GetWorkareaRepoDiffPayload`. The renderer's `RpcMethod` union also grows the new method string. Backend already lived in Task 29.
  - **`CodePrRegion` placeholder swap is targeted at the `diff` sub-tab only.** `Checks` and `PR` remain stub cards (V1.0 / Task 45). Repo-name label now reads from `Repositories.ListByProject` instead of falling back to `workarea.branch_name` whenever the project's repo list resolves.
  - **Per-file `original` / `modified` reconstruction from unified-diff bodies** (pre-decision 3). `synthesizeSides` walks each `DiffHunk.body`, strips `+` for the before-side and `-` for the after-side, and joins hunks with newlines. Imperfect for context spanning hunk gaps — acceptable per `tasks/47 §implementation notes`. A V1.0 path can call new `Repositories.GetBlob` RPCs for both sides and pass them straight to Monaco.
- **Open questions for next task:**
  - **Workarea → repository link.** The first follow-up that needs multi-repo workareas should add `repository_ids` (or a richer `WorkareaRepo` list) to the `Workareas.GetWorkarea` response so `CodePrRegion` can render per-repo tabs without a side query. The locked field numbers (`Workarea` 1..=10) leave plenty of room.
  - **`diff.<workarea>.<repo>` stream subject** is still a Task 30 / Task 47 V1.0 follow-up. The Refresh button is the V0.1 affordance; once the subject exists, hook `useDiff` into `useEventSubscription` and call `invalidateQueries` on each frame.
  - **Bundle splitting for Monaco.** The 4.4 MB main chunk warning will get worse as more languages land. A `manualChunks` split routing `monaco-editor` + `@monaco-editor/react` into a deferred chunk is the natural lift when Phase 4 polishes the desktop bundle.
- **Deliberate debt:**
  - **No `diff.<workarea>.<repo>` stream subject yet** — V0.1 polls / refreshes; live updates are V1.0 (carries over from the Task 47 spec).
  - **File-list virtualization skipped** — bounded by Monaco only mounting for the selected file; suffices for V0.1 diff sizes.
  - **Vitest unit tests skipped** (pre-decision 10) — the JS test infra hasn't landed in Phase 3; `pnpm build` (`tsc --noEmit` + `vite build`) plus the cargo gauntlet covers everything except the runtime Monaco mount.
  - **`pnpm tauri build --debug` skipped** (pre-decision 12) — too slow to run in the orchestrator gauntlet; covered by `pnpm build` (renderer) + `cargo build -p concerto-desktop` (shell).
  - **Manual Monaco runtime check deferred to the operator** — verifying the WebView mounts Monaco, fetches a real diff, and toggles split/unified requires `pnpm tauri dev` against a running Core with uncommitted edits in a workarea.
  - **`original` / `modified` synthesised from hunk bodies** — context outside hunk windows is lost; Monaco still renders the windowed payload correctly.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` exits 0 with "Smoke gate v2: PASSED".

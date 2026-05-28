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
- [ ] Verification commands pass.
- [ ] Diff renders correctly for a real workarea.
- [ ] Side-by-side / inline toggle works.
- [ ] Large-diff virtualization verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no `diff.<workarea>.<repo>` stream subject yet — V0.1 polls; live updates are V1.0.
- **Smoke-gate state:** unchanged.

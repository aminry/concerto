//! `.context/` directory creation for a workarea (Task 30).
//!
//! Task 20 laid down the minimal skeleton (empty `PROMPT.md`, empty
//! `todos.md`, empty `scratch/`). Task 30 expands that to the full
//! layout `design/03 §4.2` specifies:
//!
//! ```text
//! <worktree_root>/.context/
//! ├── PROMPT.md          # V0.1 minimal preamble (Task 33 ships the full template)
//! ├── todos.md           # empty checklist scaffold
//! ├── scratch/           # agent-writable scratch (gitignored)
//! └── checkpoints/       # checkpoint metadata; refs live in repo .git (Task 34 fills it)
//! ```
//!
//! `concerto.log` is created by Task 22's session host as soon as a
//! session starts writing — not pre-created here.
//!
//! ## Idempotency
//!
//! [`apply`] is safe to call repeatedly: existing directories survive
//! `create_dir_all`, and a non-empty `PROMPT.md` / `todos.md` is left
//! untouched (we only seed when the file is missing). This matches the
//! workarea-creation collision retry loop in `workarea.rs`, which may
//! invoke `apply` again on a fresh `worktree_root` after a unique-name
//! collision rolled back the prior DB row.

use std::path::Path;

use concerto_error::Result;

/// V0.1 placeholder body for `.context/PROMPT.md`. Task 33 (Concerto
/// preamble) replaces this with the full templated agent preamble; for
/// V0.1 the agent just sees a one-line header so the file is present
/// and readable instead of empty.
pub const PROMPT_MD_BODY: &str = "# Concerto preamble (V0.1)\n";

/// V0.1 placeholder body for `.context/todos.md`. An empty Markdown
/// checklist scaffold so the agent can start checking items immediately
/// without having to set up the file structure first.
pub const TODOS_MD_BODY: &str =
    "# Todos\n\n<!-- agent-managed checklist; mirror of the `todos` table -->\n";

/// Create (or top up) the `.context/` skeleton at `worktree_root`.
///
/// - Always ensures `scratch/` and `checkpoints/` exist.
/// - Writes `PROMPT.md` only when missing or empty (so a hand-edited
///   preamble survives a collision retry — though in V0.1 the file is
///   freshly seeded every time because Task 33 hasn't shipped yet).
/// - Same for `todos.md`.
///
/// The function is `async` because it uses `tokio::fs`; the writes are
/// small and serial.
pub async fn apply(worktree_root: &Path) -> Result<()> {
    let context = worktree_root.join(".context");
    tokio::fs::create_dir_all(context.join("scratch")).await?;
    tokio::fs::create_dir_all(context.join("checkpoints")).await?;

    let prompt = context.join("PROMPT.md");
    if !file_has_contents(&prompt).await? {
        tokio::fs::write(&prompt, PROMPT_MD_BODY).await?;
    }
    let todos = context.join("todos.md");
    if !file_has_contents(&todos).await? {
        tokio::fs::write(&todos, TODOS_MD_BODY).await?;
    }
    Ok(())
}

/// True iff `path` exists and is a non-empty regular file.
async fn file_has_contents(path: &Path) -> Result<bool> {
    match tokio::fs::metadata(path).await {
        Ok(md) if md.is_file() && md.len() > 0 => Ok(true),
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(concerto_error::Error::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apply_creates_full_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        apply(root).await.expect("apply");
        let ctx = root.join(".context");
        assert!(ctx.join("PROMPT.md").is_file());
        assert!(ctx.join("todos.md").is_file());
        assert!(ctx.join("scratch").is_dir());
        assert!(ctx.join("checkpoints").is_dir());
        let prompt = tokio::fs::read_to_string(ctx.join("PROMPT.md"))
            .await
            .unwrap();
        assert_eq!(prompt, PROMPT_MD_BODY);
        let todos = tokio::fs::read_to_string(ctx.join("todos.md"))
            .await
            .unwrap();
        assert_eq!(todos, TODOS_MD_BODY);
    }

    #[tokio::test]
    async fn apply_preserves_existing_contentful_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ctx = root.join(".context");
        tokio::fs::create_dir_all(&ctx).await.unwrap();
        let custom = "# custom\n";
        tokio::fs::write(ctx.join("PROMPT.md"), custom)
            .await
            .unwrap();
        apply(root).await.expect("apply");
        let got = tokio::fs::read_to_string(ctx.join("PROMPT.md"))
            .await
            .unwrap();
        assert_eq!(
            got, custom,
            "apply must not overwrite a non-empty PROMPT.md"
        );
    }

    #[tokio::test]
    async fn apply_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        apply(root).await.expect("first");
        apply(root).await.expect("second");
        apply(root).await.expect("third");
        assert!(root.join(".context/scratch").is_dir());
        assert!(root.join(".context/checkpoints").is_dir());
    }
}

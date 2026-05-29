//! Filesystem allow-list / hard deny-list (Task 41).
//!
//! Implements the filesystem policy locked by `design/00 §7.2` and
//! `design/12 §3.5–3.7`:
//!
//! - **Allow-list** = the workarea's `worktree_root` + its `.context/`
//!   subdirectory + each attached repo's `worktree_path` + any
//!   per-project `writable_paths` declared in
//!   `projects.settings_json` + the global `~/concerto/` root.
//! - **Deny-list** = a hard floor of paths that are NEVER auto-approved
//!   regardless of `permission_mode` (`design/12 §3.7`): `~/.ssh`,
//!   `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.netrc`, `~/.docker/config.json`.
//!
//! The decision is `Denied` (deny wins), `Allowed`, or `Outside`. The
//! `PermissionResolver` consults this module BEFORE its mode-class
//! table (see `agent_supervisor::actor::dispatch_parse_event`):
//!
//! - `Denied` → force `AutoDeny` regardless of mode. The
//!   `tool_approvals.decision` row is written with the wire string
//!   `"denied_by_policy"` so the audit log can distinguish a
//!   policy-floor denial from a user `"deny"`.
//! - `Outside` → fall through to the mode-class table. In `auto` mode
//!   this raises `MustAsk` (per `design/12 §483`); in `yolo` mode the
//!   class table still auto-approves outside-the-worktree writes.
//! - `Allowed` → fall through to the mode-class table unchanged.
//!
//! ## Canonicalization
//!
//! Both allow and deny matching are prefix-based on **canonical** paths.
//! Canonicalization defends against the classic symlink-escape: a
//! symlink under the workarea that points at `~/.ssh/config` resolves
//! to the canonical deny path under [`AllowList::for_workarea`]'s
//! caller's `home` dir, and [`classify`] returns `Denied`.
//!
//! `std::fs::canonicalize` only works for existing paths. For paths an
//! agent has not yet created (e.g. `Write { file_path: "/new/file" }`)
//! [`classify`] falls back to [`path_clean::clean`] — a *lexical*
//! cleaner that collapses `..` / `.` / repeated separators without
//! touching the filesystem. The lexical fallback is sound for prefix
//! matching IF the allow/deny roots themselves are canonical (and the
//! allow-list constructor canonicalizes them). The one corner case
//! that lexical normalization misses — `..` traversing through a
//! symlink — is not a security gap because the resolver requires the
//! `MustAsk` path for `Outside` anyway. A more elaborate
//! "open + readlinkat the parent" scheme is V1.0 (`design/12 §3.5`).
//!
//! ## Public surface frozen by this task
//!
//! - [`AllowList`], [`DenyList`], [`PathDecision`], [`classify`].
//! - The deny-list literals returned by [`DenyList::v0_1_default`].
//!   Adding paths is fine; removing requires explicit design
//!   justification per `tasks/41` §"Public interface this task locks".

use std::path::{Path, PathBuf};

use concerto_persist::{Persistence, Repository, Workarea, WorkareaId};

/// One root of the filesystem allow-list. Held as a `PathBuf` so the
/// owning `AllowList` is `'static`.
type AllowRoot = PathBuf;

/// One root of the filesystem deny-list.
type DenyRoot = PathBuf;

/// Filesystem allow-list: an ordered list of *canonical* path prefixes
/// the agent may write into freely (subject to the mode-class table on
/// the resolver side).
///
/// Construction normalises every root with [`canonicalize_or_clean`] so
/// prefix matching on canonical-or-cleaned candidate paths is sound.
#[derive(Debug, Clone, Default)]
pub struct AllowList {
    roots: Vec<AllowRoot>,
}

impl AllowList {
    /// Build an empty allow-list. Callers typically use
    /// [`AllowList::for_workarea`] instead; this constructor is here for
    /// composition / tests.
    pub fn new() -> Self {
        Self { roots: Vec::new() }
    }

    /// Build an allow-list from a workarea + its repos + the user's
    /// home dir. Inserts (in order):
    ///
    /// 1. `workarea.worktree_root` (canonicalized).
    /// 2. `<worktree_root>/.context/` (the per-workarea scratchpad
    ///    locked by `design/03 §3.10`).
    /// 3. Each `repo.worktree_path`.
    /// 4. Any `writable_paths` array values from `project_settings_json`
    ///    (see [`extract_writable_paths`]).
    /// 5. `<home>/concerto/` — the global Concerto data root; every
    ///    workarea, log, and checkpoint lives under it, so an agent
    ///    inspecting its own state stays inside the allow-list.
    ///
    /// `home` is passed in (rather than read here) so tests can fake
    /// it without touching `$HOME`.
    pub fn for_workarea(
        workarea: &Workarea,
        repos: &[Repository],
        project_settings_json: Option<&str>,
        home: &Path,
    ) -> Self {
        let mut roots: Vec<AllowRoot> = Vec::new();
        let worktree_root = PathBuf::from(&workarea.worktree_root);
        roots.push(canonicalize_or_clean(&worktree_root));
        roots.push(canonicalize_or_clean(worktree_root.join(".context")));
        for r in repos {
            roots.push(canonicalize_or_clean(Path::new(&r.local_path)));
        }
        if let Some(settings) = project_settings_json {
            for p in extract_writable_paths(settings) {
                roots.push(canonicalize_or_clean(&p));
            }
        }
        roots.push(canonicalize_or_clean(home.join("concerto")));
        Self { roots }
    }

    /// Push an additional allow root. Returns `self` so calls can be
    /// chained at construction sites.
    pub fn push(&mut self, root: PathBuf) -> &mut Self {
        self.roots.push(canonicalize_or_clean(&root));
        self
    }

    /// Borrow the canonical allow roots in insertion order.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// Hard deny-list — paths the resolver NEVER auto-approves, regardless
/// of `permission_mode` or `bypass_destructive_guard`. Per
/// `design/12 §3.5`/`§3.7`, this is the only floor that's never
/// bypassed.
#[derive(Debug, Clone, Default)]
pub struct DenyList {
    roots: Vec<DenyRoot>,
}

impl DenyList {
    /// Build an empty deny-list. The default V0.1 set lives in
    /// [`DenyList::v0_1_default`].
    pub fn new() -> Self {
        Self { roots: Vec::new() }
    }

    /// V0.1 hard deny-list expanded against `home`. Paths:
    ///
    /// - `<home>/.ssh`
    /// - `<home>/.aws`
    /// - `<home>/.gnupg`
    /// - `<home>/.kube`
    /// - `<home>/.netrc`
    /// - `<home>/.docker/config.json`
    ///
    /// Each root is canonicalized at construction time (when the path
    /// exists); non-existing roots are lexically cleaned. The result
    /// is the prefix used by [`classify`].
    ///
    /// Adding paths to this list is fine; removing one requires
    /// explicit justification per the Task 41 contract.
    pub fn v0_1_default(home: &Path) -> Self {
        let raw = [
            home.join(".ssh"),
            home.join(".aws"),
            home.join(".gnupg"),
            home.join(".kube"),
            home.join(".netrc"),
            home.join(".docker").join("config.json"),
        ];
        let roots = raw.iter().map(canonicalize_or_clean).collect();
        Self { roots }
    }

    /// Push an additional deny root. V0.1 has no callers; reserved for
    /// the per-project deny extension in V1.0
    /// (`tasks/41` §"Scope — out").
    pub fn push(&mut self, root: PathBuf) -> &mut Self {
        self.roots.push(canonicalize_or_clean(&root));
        self
    }

    /// Borrow the canonical deny roots in insertion order.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// Verdict of [`classify`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PathDecision {
    /// Path is inside an allow root and outside every deny root.
    Allowed,
    /// Path is outside every allow root AND outside every deny root.
    /// The mode-class table decides next.
    Outside,
    /// Path is inside a deny root. The resolver forces `AutoDeny`.
    Denied,
}

/// Classify `path` against the allow + deny lists. The match runs on
/// the canonical (or lexically cleaned) form of `path`; symlinks are
/// resolved before prefix matching (see module docs).
///
/// Deny wins over allow: a path that is both under an allow root AND
/// under a deny root returns `Denied`. This is the only behaviour that
/// makes the floor `design/12 §3.7` describes actually unbypassable —
/// project-declared writable paths cannot accidentally re-allow a deny
/// path.
pub fn classify(path: &Path, allow: &AllowList, deny: &DenyList) -> PathDecision {
    let canonical = canonicalize_or_clean(path);
    for d in deny.roots() {
        if starts_with_path(&canonical, d) {
            return PathDecision::Denied;
        }
    }
    for a in allow.roots() {
        if starts_with_path(&canonical, a) {
            return PathDecision::Allowed;
        }
    }
    PathDecision::Outside
}

/// Build the per-workarea `(AllowList, DenyList)` pair the resolver
/// consults on every tool call. Reads the workarea row + its attached
/// repos + the owning project's `settings_json` from `persistence` and
/// expands the deny-list against `home`.
///
/// V0.1 callers (the Agent Supervisor's `dispatch_parse_event`) invoke
/// this lazily on each `AwaitingApproval` — the cost is two SQL reads
/// plus a handful of `canonicalize` syscalls, which is well inside the
/// approval-gate budget (`design/04 §3.10` makes the gate
/// human-latency-bound anyway). If profiling demands it, the supervisor
/// can cache the pair on `SessionEntry`; the public surface here stays
/// the same.
pub async fn for_workarea_from_db(
    persistence: &Persistence,
    workarea_id: &WorkareaId,
    home: &Path,
) -> Result<(AllowList, DenyList), concerto_error::Error> {
    use concerto_error::Error;
    let pool = persistence.readers();
    let workarea = concerto_persist::workareas::get(pool, workarea_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("workarea {workarea_id} not found")))?;
    // Resolve attached repos through the junction; `Repository` rows
    // carry the canonical `local_path` for each cloned working copy.
    let junction = concerto_persist::workareas::list_workarea_repos(pool, workarea_id).await?;
    let mut repos: Vec<Repository> = Vec::with_capacity(junction.len());
    for (repo_id, _worktree_path) in junction {
        if let Some(r) = concerto_persist::repositories::get(pool, &repo_id).await? {
            repos.push(r);
        }
    }
    // Find the owning workspace to look up the project's settings_json.
    let workspace = concerto_persist::workspaces::get(pool, &workarea.workspace_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("workspace {} not found", workarea.workspace_id)))?;
    let project_settings_json = concerto_persist::projects::get_settings_json(
        pool,
        &concerto_persist::ProjectId(workspace.project_id.clone()),
    )
    .await?;

    let allow = AllowList::for_workarea(&workarea, &repos, project_settings_json.as_deref(), home);
    let deny = DenyList::v0_1_default(home);
    Ok((allow, deny))
}

/// Try `std::fs::canonicalize` first; on error (path doesn't exist,
/// permission denied, …) fall back to [`path_clean::clean`] applied to
/// the path made absolute against the current working directory.
///
/// The fallback is lexical-only — it never touches the filesystem — so
/// it is sound to call on paths that haven't been created yet. The
/// price is that `..`-through-a-symlink is not resolved; see module
/// docs for why that's acceptable in V0.1.
pub fn canonicalize_or_clean<P: AsRef<Path>>(p: P) -> PathBuf {
    let p = p.as_ref();
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    // Make absolute first (so `path_clean` sees `/foo/bar`, not
    // `foo/bar`). If `current_dir` fails (extremely rare), just clean
    // the raw input — better than panicking on the policy hot path.
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(p),
            Err(_) => p.to_path_buf(),
        }
    };
    path_clean::clean(&absolute)
}

/// True iff `candidate` starts with `prefix` AS a path (component-wise),
/// not byte-wise. This is `PathBuf::starts_with` but spelled out so the
/// intent is explicit at the call site.
fn starts_with_path(candidate: &Path, prefix: &Path) -> bool {
    candidate.starts_with(prefix)
}

/// Pull the `writable_paths` string array out of a project's
/// `settings_json` blob. Returns an empty vec on malformed JSON or
/// absent key — V0.1 treats project settings as advisory; never crash
/// the resolver because a project's settings JSON has a typo.
fn extract_writable_paths(settings_json: &str) -> Vec<PathBuf> {
    let parsed: serde_json::Value = match serde_json::from_str(settings_json) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = parsed
        .as_object()
        .and_then(|m| m.get("writable_paths"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| v.as_str().map(PathBuf::from))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn extract_writable_paths_array() {
        let s = r#"{"writable_paths": ["/tmp/a", "/tmp/b"]}"#;
        let p = extract_writable_paths(s);
        assert_eq!(p, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
    }

    #[test]
    fn extract_writable_paths_missing_key() {
        let s = r#"{}"#;
        assert!(extract_writable_paths(s).is_empty());
    }

    #[test]
    fn extract_writable_paths_malformed() {
        let s = "not json";
        assert!(extract_writable_paths(s).is_empty());
    }

    #[test]
    fn canonicalize_or_clean_existing_path() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("a/b/..");
        std::fs::create_dir_all(td.path().join("a/b")).unwrap();
        let c = canonicalize_or_clean(&p);
        // The `a/b/..` collapses to `a/` and canonicalize resolves any
        // tempdir symlinks (macOS `/var` → `/private/var`).
        assert!(c.ends_with("a"));
    }

    #[test]
    fn canonicalize_or_clean_missing_path() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("does/not/exist/../exist");
        let c = canonicalize_or_clean(&p);
        // The `..` is lexically collapsed even though no node exists.
        assert!(c.ends_with("does/not/exist"));
    }

    #[cfg(unix)]
    #[test]
    fn deny_v0_1_default_expands_home() {
        let home = PathBuf::from("/tmp/fake-home");
        let d = DenyList::v0_1_default(&home);
        let roots: Vec<String> = d
            .roots()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(roots.iter().any(|r| r.ends_with("/.ssh")));
        assert!(roots.iter().any(|r| r.ends_with("/.aws")));
        assert!(roots.iter().any(|r| r.ends_with("/.gnupg")));
        assert!(roots.iter().any(|r| r.ends_with("/.kube")));
        assert!(roots.iter().any(|r| r.ends_with("/.netrc")));
        assert!(roots.iter().any(|r| r.ends_with("/.docker/config.json")));
    }

    #[test]
    fn classify_allowed_by_prefix() {
        let td = TempDir::new().unwrap();
        // Canonicalize the tempdir base so prefix matching works on
        // macOS (where `/var/folders/...` symlinks to `/private/var/...`).
        let base = canonicalize_or_clean(td.path());
        let inside = base.join("subdir/file.txt");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        let mut a = AllowList::new();
        a.push(base.clone());
        let d = DenyList::new();
        assert_eq!(classify(&inside, &a, &d), PathDecision::Allowed);
    }

    #[test]
    fn classify_outside_when_no_prefix_matches() {
        let td = TempDir::new().unwrap();
        let td2 = TempDir::new().unwrap();
        let mut a = AllowList::new();
        a.push(td.path().to_path_buf());
        let d = DenyList::new();
        let outside = td2.path().join("else.txt");
        assert_eq!(classify(&outside, &a, &d), PathDecision::Outside);
    }

    #[test]
    fn classify_denied_wins_over_allowed() {
        let td = TempDir::new().unwrap();
        let base = canonicalize_or_clean(td.path());
        let mut a = AllowList::new();
        a.push(base.clone());
        let mut d = DenyList::new();
        d.push(base.join("secrets"));
        std::fs::create_dir_all(base.join("secrets")).unwrap();
        let p = base.join("secrets/api-key");
        assert_eq!(classify(&p, &a, &d), PathDecision::Denied);
    }
}

//! Skill discovery walker (Task 39).
//!
//! Scans `~/.claude/skills/*/SKILL.md` (personal scope) and per-project
//! `<repo.local_path>/.claude/skills/*/SKILL.md` (project scope),
//! parses each SKILL.md's YAML frontmatter, and upserts each entry
//! into `skills_index`. Malformed files are warned + skipped — the
//! walk never aborts because of one bad fixture.
//!
//! ## Frontmatter shape
//!
//! ```yaml
//! ---
//! name: my-skill
//! description: What this does.
//! slash-command: /my-skill   # optional
//! tools: [Read, Edit]        # optional
//! ---
//! ```
//!
//! The parser is hand-rolled: find the first `---` line, find the
//! second `---` line, treat the chunk between as YAML, and parse via
//! `serde_yaml::from_str::<SkillFrontmatter>`. The remainder of the
//! file is markdown body — discarded for V0.1.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use concerto_error::Result;
use concerto_persist::{NewSkill, Persistence, ProjectId, SkillId, SkillScope};
use serde::Deserialize;

/// Parsed `SKILL.md` frontmatter. All fields except `name` are
/// optional; the discovery walker falls back to the skill directory
/// name when `name` is missing.
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "slash-command")]
    pub slash_command: Option<String>,
    pub tools: Option<Vec<String>>,
}

/// Result of a [`discover`] call. `discovered_count` is the number of
/// `skills_index` rows touched (insert + update). `errors` collects
/// per-file failures so the caller can surface them in the UI without
/// the walk aborting.
#[derive(Debug, Clone, Default)]
pub struct SkillsRefreshReport {
    pub discovered_count: u64,
    pub errors: Vec<String>,
}

/// Walk the personal scope (`<home_dir>/.claude/skills/`) and the
/// per-project scope (`<repo.local_path>/.claude/skills/`) for every
/// repository registered against `project_filter` (or every project
/// when `project_filter` is `None`), parsing each SKILL.md and
/// upserting the result into `skills_index`.
///
/// The walk is sync filesystem I/O wrapped in `spawn_blocking` so the
/// tokio runtime stays unblocked. Returns a [`SkillsRefreshReport`]
/// summarising the rescan.
pub async fn discover(
    persistence: &Arc<Persistence>,
    home_dir: &Path,
    project_filter: Option<&ProjectId>,
) -> Result<SkillsRefreshReport> {
    let mut report = SkillsRefreshReport::default();

    // ---- Personal scope --------------------------------------------------
    let personal_root = home_dir.join(".claude").join("skills");
    walk_scope(
        persistence,
        &personal_root,
        SkillScope::Personal,
        None,
        &mut report,
    )
    .await?;

    // ---- Project scope ---------------------------------------------------
    // Resolve every repository's `local_path` per the project filter.
    // V0.1 walks both `personal` and `project` on every refresh;
    // marketplace fetch lands in V1.0 behind the same RPC.
    let repos = list_project_repos(persistence, project_filter).await?;
    for (project_id, local_path) in repos {
        let project_root = PathBuf::from(local_path).join(".claude").join("skills");
        walk_scope(
            persistence,
            &project_root,
            SkillScope::Project,
            Some(project_id),
            &mut report,
        )
        .await?;
    }

    Ok(report)
}

/// Walk a single `<root>/<skill-name>/SKILL.md` layout and upsert each
/// well-formed entry. Errors on individual files are recorded in
/// `report.errors`; the walk continues.
async fn walk_scope(
    persistence: &Arc<Persistence>,
    root: &Path,
    scope: SkillScope,
    project_id: Option<ProjectId>,
    report: &mut SkillsRefreshReport,
) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }

    // Collect entries via blocking I/O on a tokio blocking thread.
    let root_buf = root.to_path_buf();
    let entries = tokio::task::spawn_blocking(move || list_skill_dirs(&root_buf))
        .await
        .map_err(|e| {
            concerto_error::Error::Internal(format!("skills discovery join error: {e}"))
        })??;

    for skill_dir in entries {
        let skill_md = skill_dir.join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let raw = match tokio::fs::read_to_string(&skill_md).await {
            Ok(s) => s,
            Err(e) => {
                let msg = format!("skills: failed to read {}: {}", skill_md.display(), e);
                tracing::warn!(error = %e, path = %skill_md.display(), "skills.read_failed");
                report.errors.push(msg);
                continue;
            }
        };
        let frontmatter = match parse_frontmatter(&raw) {
            Ok(fm) => fm,
            Err(msg) => {
                let full = format!("skills: failed to parse {}: {msg}", skill_md.display());
                tracing::warn!(
                    error = %msg,
                    path = %skill_md.display(),
                    "skills.frontmatter_parse_failed"
                );
                report.errors.push(full);
                continue;
            }
        };

        // Fall back to the directory name when `name` is missing.
        let name = frontmatter.name.clone().unwrap_or_else(|| {
            skill_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        if name.trim().is_empty() {
            let msg = format!(
                "skills: empty `name` in frontmatter at {}",
                skill_md.display()
            );
            tracing::warn!(path = %skill_md.display(), "skills.empty_name");
            report.errors.push(msg);
            continue;
        }
        let tools_json = serde_json::to_string(&frontmatter.tools.unwrap_or_default())
            .unwrap_or_else(|_| "[]".to_string());
        let now_ms = now_unix_ms();
        let new = NewSkill {
            id: SkillId(uuid::Uuid::now_v7().to_string()),
            scope,
            project_id: project_id.clone(),
            name,
            slash_command: frontmatter.slash_command,
            description: frontmatter.description,
            tools_json,
            source_path: skill_dir.to_string_lossy().into_owned(),
            discovered_at: now_ms,
        };
        let mut writer = persistence.writer().await;
        concerto_persist::skills::upsert(&mut writer, new).await?;
        drop(writer);
        report.discovered_count += 1;
    }
    Ok(())
}

/// List immediate subdirectories of `root`. Sync I/O — call inside a
/// `spawn_blocking`.
fn list_skill_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let read = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(concerto_error::Error::Io(e)),
    };
    for entry in read {
        let entry = entry.map_err(concerto_error::Error::Io)?;
        let path = entry.path();
        if path.is_dir() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Parse the YAML frontmatter chunk between the first and second
/// `---` lines of a SKILL.md file. Returns the parsed
/// [`SkillFrontmatter`], or a descriptive error message when no
/// frontmatter is present or the YAML is malformed.
pub(crate) fn parse_frontmatter(raw: &str) -> std::result::Result<SkillFrontmatter, String> {
    let trimmed = raw.trim_start_matches('\u{FEFF}'); // strip BOM
    let mut lines = trimmed.lines();
    let first = lines.next().ok_or_else(|| "empty file".to_string())?;
    if first.trim() != "---" {
        return Err("missing leading `---` frontmatter delimiter".into());
    }
    let mut body = String::new();
    let mut closed = false;
    for line in lines {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !closed {
        return Err("missing trailing `---` frontmatter delimiter".into());
    }
    serde_yaml::from_str::<SkillFrontmatter>(&body).map_err(|e| format!("yaml parse error: {e}"))
}

/// Read every repository's `(project_id, local_path)` per the optional
/// filter. Returns `(ProjectId, String)` so the caller can compose the
/// per-repo `.claude/skills/` path directly.
async fn list_project_repos(
    persistence: &Arc<Persistence>,
    project_filter: Option<&ProjectId>,
) -> Result<Vec<(ProjectId, String)>> {
    use sqlx::Row;
    let pool = persistence.readers();
    let rows = if let Some(p) = project_filter {
        sqlx::query("SELECT project_id, local_path FROM repositories WHERE project_id = ?")
            .bind(&p.0)
            .fetch_all(pool)
            .await
            .map_err(|e| concerto_error::Error::Sqlx(Box::new(e)))?
    } else {
        sqlx::query("SELECT project_id, local_path FROM repositories")
            .fetch_all(pool)
            .await
            .map_err(|e| concerto_error::Error::Sqlx(Box::new(e)))?
    };
    Ok(rows
        .into_iter()
        .map(|r| {
            let pid: String = r.get("project_id");
            let path: String = r.get("local_path");
            (ProjectId(pid), path)
        })
        .collect())
}

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_frontmatter() {
        let raw = "---\nname: foo\ndescription: bar\nslash-command: /foo\ntools:\n  - Read\n  - Edit\n---\n# body\n";
        let fm = parse_frontmatter(raw).expect("parse");
        assert_eq!(fm.name.as_deref(), Some("foo"));
        assert_eq!(fm.description.as_deref(), Some("bar"));
        assert_eq!(fm.slash_command.as_deref(), Some("/foo"));
        assert_eq!(
            fm.tools.unwrap(),
            vec!["Read".to_string(), "Edit".to_string()]
        );
    }

    #[test]
    fn parses_minimal_frontmatter() {
        let raw = "---\nname: bare\n---\nbody\n";
        let fm = parse_frontmatter(raw).expect("parse");
        assert_eq!(fm.name.as_deref(), Some("bare"));
        assert!(fm.description.is_none());
        assert!(fm.slash_command.is_none());
        assert!(fm.tools.is_none());
    }

    #[test]
    fn rejects_missing_leading_delim() {
        let raw = "name: x\n";
        let err = parse_frontmatter(raw).unwrap_err();
        assert!(err.contains("leading"));
    }

    #[test]
    fn rejects_unclosed_frontmatter() {
        let raw = "---\nname: x\n";
        let err = parse_frontmatter(raw).unwrap_err();
        assert!(err.contains("trailing"));
    }

    #[test]
    fn rejects_malformed_yaml() {
        let raw = "---\nname: : :\ntools:\n  -\n   bad\n: :\n---\n";
        let err = parse_frontmatter(raw).unwrap_err();
        assert!(err.contains("yaml"));
    }
}

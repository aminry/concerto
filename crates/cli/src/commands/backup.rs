//! `concerto backup` — capture a portable snapshot of the local Concerto
//! state (design/09 §6.4).
//!
//! Unlike every other `concerto` subcommand, `backup` does **not** dial the
//! Core over its UDS socket: it operates on the local DB **file** directly so
//! it works even when no Core is running. It produces, under `<out>/`:
//!
//!   * `concerto.db` — a hot-consistent SQLite snapshot via `VACUUM INTO`.
//!   * `worktrees.tar` — (with `--with-worktrees`) a stream-tarred copy of the
//!     worktree directory tree.
//!   * `audit.jsonl` — (with `--audit-from`/`--audit-to`) the JSONL audit
//!     records whose `at` timestamp falls in `[from, to]`.
//!   * `manifest.json` — what was included, plus UTC ISO-8601 timestamps and
//!     versions.
//!
//! This `<out>/` layout is **FROZEN** (Task 111): a future `concerto restore`
//! reads exactly these filenames.
//!
//! ## Path resolution (single source of truth)
//!
//! The DB path is resolved the same way the Core resolves it
//! (`crates/core/src/runtime.rs`), so backup always targets the file the Core
//! actually writes:
//!
//!   1. `$CONCERTO_DB_PATH` if set and non-empty.
//!   2. else `<data_dir>/concerto.db`, where `data_dir` is `$CONCERTO_DATA_DIR`
//!      if set, else `$CONCERTO_HOME/concerto` if set, else `<home>/concerto`
//!      (matching [`concerto_persist::PersistenceConfig::default_for_user`]).
//!
//! `$CONCERTO_HOME` is honored as the scratch-home convention the smoke gate
//! uses (it sets `CONCERTO_DATA_DIR=$CONCERTO_HOME/concerto`); supporting it
//! directly means a bare `CONCERTO_HOME=… concerto backup` also resolves the
//! right tree.
//!
//! The worktree tree and the audit JSONL live under the same `data_dir`
//! (`<data_dir>/workspaces/` and `<data_dir>/audit/` respectively), per
//! design/09 §4 and `crates/core/src/audit/jsonl.rs`.
//!
//! ## Concurrent-write behavior
//!
//! `VACUUM INTO` opens the source DB read-only and takes a SQLite read lock
//! for the duration of the copy; SQLite's WAL mode lets writers continue
//! against the live DB while the snapshot is produced, and the snapshot
//! reflects a single consistent point-in-time (the read transaction's
//! snapshot). We therefore do **not** refuse to run while a Core is live —
//! the snapshot is internally consistent regardless. We open the source
//! read-only (`?mode=ro`) so backup can never mutate the live DB.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

use super::{CommandError, OutputFormat};

/// Frozen output filenames under `<out>/` (Task 111). A future `concerto
/// restore` reads exactly these.
const DB_SNAPSHOT_NAME: &str = "concerto.db";
const WORKTREES_TAR_NAME: &str = "worktrees.tar";
const AUDIT_JSONL_NAME: &str = "audit.jsonl";
const MANIFEST_NAME: &str = "manifest.json";

/// `manifest.json` schema (design/09 §6.4). All timestamps are UTC ISO-8601.
#[derive(Debug, Serialize)]
struct Manifest {
    /// Manifest format version. Bump only on a breaking layout change; the
    /// `<out>/` filenames are frozen, so this starts at 1.
    manifest_version: u32,
    /// The `concerto` CLI version that produced this backup.
    concerto_version: String,
    /// When the backup was taken (UTC ISO-8601, millisecond precision).
    created_at: String,
    /// Absolute source DB path the snapshot was taken from.
    source_db_path: String,
    /// What this backup includes.
    included: Included,
}

/// The artifact inventory recorded in the manifest.
#[derive(Debug, Serialize)]
struct Included {
    /// Always present — the `VACUUM INTO` snapshot filename.
    db_snapshot: String,
    /// Present (the tar filename) only when `--with-worktrees` was passed.
    worktrees_tar: Option<String>,
    /// Present (the JSONL filename) only when an audit range was requested.
    audit_jsonl: Option<String>,
    /// The inclusive audit range `[from, to]` if one was requested.
    audit_from: Option<String>,
    audit_to: Option<String>,
    /// Number of audit records copied into `audit.jsonl` (0 if no range).
    audit_records: u64,
}

/// Parsed `concerto backup` arguments (after clap), resolved against the
/// environment.
#[derive(Debug)]
pub struct BackupArgs {
    /// Output directory. Created if missing.
    pub out: PathBuf,
    /// Tar the worktree directory tree into `<out>/worktrees.tar`.
    pub with_worktrees: bool,
    /// Inclusive lower bound on the audit `at` timestamp (ISO-8601). When
    /// either bound is set, an `audit.jsonl` is produced.
    pub audit_from: Option<String>,
    /// Inclusive upper bound on the audit `at` timestamp (ISO-8601).
    pub audit_to: Option<String>,
}

/// Run `concerto backup`.
pub async fn run(args: BackupArgs, format: OutputFormat) -> Result<(), CommandError> {
    let paths = ConcertoPaths::resolve()?;

    tokio::fs::create_dir_all(&args.out)
        .await
        .map_err(|source| {
            CommandError::Backup(BackupError::CreateOut {
                path: args.out.clone(),
                source,
            })
        })?;

    // 1. DB snapshot via VACUUM INTO (hot-consistent; read-only source).
    let snapshot_path = args.out.join(DB_SNAPSHOT_NAME);
    vacuum_into(&paths.db_path, &snapshot_path).await?;

    // 2. Optional worktree tarball (streamed; never read whole into memory).
    let worktrees_tar = if args.with_worktrees {
        let tar_path = args.out.join(WORKTREES_TAR_NAME);
        tar_worktrees(&paths.worktrees_dir, &tar_path).await?;
        Some(WORKTREES_TAR_NAME.to_string())
    } else {
        None
    };

    // 3. Optional audit-range export.
    let (audit_jsonl, audit_records) = if args.audit_from.is_some() || args.audit_to.is_some() {
        let audit_path = args.out.join(AUDIT_JSONL_NAME);
        let n = export_audit_range(
            &paths.audit_dir,
            &audit_path,
            args.audit_from.as_deref(),
            args.audit_to.as_deref(),
        )
        .await?;
        (Some(AUDIT_JSONL_NAME.to_string()), n)
    } else {
        (None, 0)
    };

    // 4. Manifest.
    let manifest = Manifest {
        manifest_version: 1,
        concerto_version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: now_iso8601_utc(),
        source_db_path: paths.db_path.display().to_string(),
        included: Included {
            db_snapshot: DB_SNAPSHOT_NAME.to_string(),
            worktrees_tar,
            audit_jsonl,
            audit_from: args.audit_from.clone(),
            audit_to: args.audit_to.clone(),
            audit_records,
        },
    };
    let manifest_path = args.out.join(MANIFEST_NAME);
    write_manifest(&manifest_path, &manifest).await?;

    render(&args.out, &manifest, format)
}

/// Resolved on-disk locations the backup reads from.
struct ConcertoPaths {
    /// The live SQLite DB file (`<data_dir>/concerto.db` or `$CONCERTO_DB_PATH`).
    db_path: PathBuf,
    /// The worktree directory tree (`<data_dir>/workspaces/`).
    worktrees_dir: PathBuf,
    /// The audit JSONL directory (`<data_dir>/audit/`).
    audit_dir: PathBuf,
}

impl ConcertoPaths {
    /// Resolve the canonical DB path + sibling worktree/audit dirs the same
    /// way the Core does (see module docs). Reuses
    /// [`concerto_persist::PersistenceConfig::default_for_user`] for the
    /// `<home>/concerto/concerto.db` default so there is no second hardcoded
    /// source of truth for the home-relative layout.
    fn resolve() -> Result<Self, CommandError> {
        let data_dir = resolve_data_dir()?;

        let db_path = match non_empty_env("CONCERTO_DB_PATH") {
            Some(p) => PathBuf::from(p),
            None => data_dir.join("concerto.db"),
        };

        Ok(Self {
            db_path,
            worktrees_dir: data_dir.join("workspaces"),
            audit_dir: data_dir.join("audit"),
        })
    }
}

/// Resolve `data_dir`: `$CONCERTO_DATA_DIR` → `$CONCERTO_HOME/concerto` →
/// `<home>/concerto` (the last via `PersistenceConfig::default_for_user`, so
/// the home-relative default stays single-sourced in `crates/persist`).
fn resolve_data_dir() -> Result<PathBuf, CommandError> {
    if let Some(dir) = non_empty_env("CONCERTO_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(home) = non_empty_env("CONCERTO_HOME") {
        return Ok(PathBuf::from(home).join("concerto"));
    }
    // `db_path` is `<home>/concerto/concerto.db`; the data_dir is its parent.
    let cfg = concerto_persist::PersistenceConfig::default_for_user()
        .map_err(|e| CommandError::Backup(BackupError::ResolveDb(e.to_string())))?;
    let data_dir = cfg.db_path.parent().map(Path::to_path_buf).ok_or_else(|| {
        CommandError::Backup(BackupError::ResolveDb(
            "default DB path has no parent directory".to_string(),
        ))
    })?;
    Ok(data_dir)
}

/// Read an env var, treating unset OR empty as absent.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// Produce a hot-consistent SQLite snapshot of `source` at `dest` via
/// `VACUUM INTO`.
///
/// The source is opened **read-only** (`mode = ro`) so a live Core's DB can
/// never be mutated by a backup; WAL mode lets concurrent writers proceed
/// while the read-locked snapshot is taken (see module docs). `dest` must not
/// already exist — SQLite refuses to `VACUUM INTO` an existing file — so any
/// stale snapshot from a prior run is removed first.
async fn vacuum_into(source: &Path, dest: &Path) -> Result<(), CommandError> {
    if !source.exists() {
        return Err(CommandError::Backup(BackupError::SourceMissing {
            path: source.to_path_buf(),
        }));
    }

    // SQLite's `VACUUM INTO` errors if the destination file exists; clear any
    // leftover snapshot so re-runs are idempotent.
    if dest.exists() {
        tokio::fs::remove_file(dest).await.map_err(|source| {
            CommandError::Backup(BackupError::Io {
                what: "removing the previous snapshot",
                source,
            })
        })?;
    }

    let opts = SqliteConnectOptions::new()
        .filename(source)
        .read_only(true)
        .create_if_missing(false);

    let mut conn = opts.connect().await.map_err(|e| {
        CommandError::Backup(BackupError::OpenSource {
            path: source.to_path_buf(),
            source: Box::new(e),
        })
    })?;

    // `VACUUM INTO` takes a bind-unfriendly path literal; SQLite has no
    // parameter form for it, so we quote the path by doubling single quotes
    // (the only metacharacter inside a SQL string literal).
    let dest_str = dest.display().to_string();
    let quoted = dest_str.replace('\'', "''");
    let sql = format!("VACUUM INTO '{quoted}'");

    sqlx::query(&sql).execute(&mut conn).await.map_err(|e| {
        CommandError::Backup(BackupError::Vacuum {
            dest: dest.to_path_buf(),
            source: Box::new(e),
        })
    })?;

    conn.close().await.map_err(|e| {
        CommandError::Backup(BackupError::OpenSource {
            path: source.to_path_buf(),
            source: Box::new(e),
        })
    })?;

    Ok(())
}

/// Stream-tar the worktree directory tree at `worktrees_dir` into `tar_path`.
///
/// Uses the pure-Rust, cross-platform `tar` crate. `Builder::append_dir_all`
/// walks the tree and streams each file into the archive without reading whole
/// files into memory. A missing worktree directory is treated as "nothing to
/// archive" — an empty tar is produced so the manifest's `worktrees_tar`
/// promise always holds.
///
/// Runs on a blocking thread (`tar` + `std::fs` are synchronous) so the async
/// runtime is never blocked.
async fn tar_worktrees(worktrees_dir: &Path, tar_path: &Path) -> Result<(), CommandError> {
    let worktrees_dir = worktrees_dir.to_path_buf();
    let tar_path = tar_path.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<(), BackupError> {
        let file = std::fs::File::create(&tar_path).map_err(|source| BackupError::Io {
            what: "creating the worktrees tarball",
            source,
        })?;
        let mut builder = tar::Builder::new(file);

        if worktrees_dir.is_dir() {
            // Archive the tree under a top-level `workspaces/` entry so the
            // archive is self-describing on extraction.
            builder
                .append_dir_all("workspaces", &worktrees_dir)
                .map_err(|source| BackupError::Io {
                    what: "streaming the worktree tree into the tarball",
                    source,
                })?;
        }

        builder.finish().map_err(|source| BackupError::Io {
            what: "finalizing the worktrees tarball",
            source,
        })?;
        Ok(())
    })
    .await
    .map_err(|e| CommandError::Backup(BackupError::Join(e.to_string())))?
    .map_err(CommandError::Backup)
}

/// Copy the audit JSONL records whose `at` timestamp falls in the inclusive
/// `[from, to]` range into `dest`. Returns the number of records written.
///
/// Audit records are one JSON object per line with an `at` field formatted as
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC, fixed-width). Because that format sorts
/// lexicographically in chronological order, range filtering is a string
/// comparison — no timestamp parsing needed, and `from`/`to` may be any
/// ISO-8601 prefix (e.g. `2026-05-30`).
///
/// All `audit-*.jsonl` files in `audit_dir` are scanned (daily rotation, per
/// design/09 §3.5) in filename order — which is date order. A missing audit
/// directory yields an empty `audit.jsonl` (0 records).
///
/// Runs on a blocking thread (synchronous file I/O).
async fn export_audit_range(
    audit_dir: &Path,
    dest: &Path,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<u64, CommandError> {
    let audit_dir = audit_dir.to_path_buf();
    let dest = dest.to_path_buf();
    let from = from.map(str::to_string);
    let to = to.map(str::to_string);

    tokio::task::spawn_blocking(move || -> Result<u64, BackupError> {
        use std::io::{BufRead, BufReader, BufWriter, Write};

        let out_file = std::fs::File::create(&dest).map_err(|source| BackupError::Io {
            what: "creating the audit export file",
            source,
        })?;
        let mut writer = BufWriter::new(out_file);
        let mut written: u64 = 0;

        if audit_dir.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&audit_dir)
                .map_err(|source| BackupError::Io {
                    what: "reading the audit directory",
                    source,
                })?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("audit-") && n.ends_with(".jsonl"))
                })
                .collect();
            // Filename order == date order (daily rotation, zero-padded).
            files.sort();

            for path in files {
                let f = std::fs::File::open(&path).map_err(|source| BackupError::Io {
                    what: "opening an audit log file",
                    source,
                })?;
                for line in BufReader::new(f).lines() {
                    let line = line.map_err(|source| BackupError::Io {
                        what: "reading an audit log line",
                        source,
                    })?;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match record_at(trimmed) {
                        Some(at) if in_range(&at, from.as_deref(), to.as_deref()) => {
                            writer
                                .write_all(line.as_bytes())
                                .and_then(|()| writer.write_all(b"\n"))
                                .map_err(|source| BackupError::Io {
                                    what: "writing an audit record to the export",
                                    source,
                                })?;
                            written += 1;
                        }
                        // Records with no parseable `at` are skipped rather
                        // than aborting the whole export — a single malformed
                        // line must not lose the rest of the range.
                        _ => {}
                    }
                }
            }
        }

        writer.flush().map_err(|source| BackupError::Io {
            what: "flushing the audit export file",
            source,
        })?;
        Ok(written)
    })
    .await
    .map_err(|e| CommandError::Backup(BackupError::Join(e.to_string())))?
    .map_err(CommandError::Backup)
}

/// Extract the `at` timestamp string from one audit JSONL line, if present.
fn record_at(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value.get("at")?.as_str().map(str::to_string)
}

/// Inclusive `[from, to]` membership test on lexicographically-sortable
/// ISO-8601 timestamps. An absent bound is unbounded on that side.
fn in_range(at: &str, from: Option<&str>, to: Option<&str>) -> bool {
    if let Some(f) = from {
        if at < f {
            return false;
        }
    }
    if let Some(t) = to {
        if at > t {
            return false;
        }
    }
    true
}

/// Serialize the manifest to `dest` (pretty JSON + trailing newline).
async fn write_manifest(dest: &Path, manifest: &Manifest) -> Result<(), CommandError> {
    let mut json = serde_json::to_string_pretty(manifest)?;
    json.push('\n');
    tokio::fs::write(dest, json).await.map_err(|source| {
        CommandError::Backup(BackupError::Io {
            what: "writing manifest.json",
            source,
        })
    })
}

/// Render the result for the user.
fn render(out: &Path, manifest: &Manifest, format: OutputFormat) -> Result<(), CommandError> {
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(manifest)?);
        return Ok(());
    }

    println!("backup written to {}", out.display());
    println!(
        "  {DB_SNAPSHOT_NAME} (snapshot of {})",
        manifest.source_db_path
    );
    if let Some(tar) = &manifest.included.worktrees_tar {
        println!("  {tar}");
    }
    if let Some(audit) = &manifest.included.audit_jsonl {
        println!(
            "  {audit} ({} records in range)",
            manifest.included.audit_records
        );
    }
    println!("  {MANIFEST_NAME}");
    Ok(())
}

/// Current wall-clock time as a UTC ISO-8601 string with millisecond
/// precision (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
///
/// Uses the same `civil_from_unix` integer algorithm the Core's audit writer
/// uses (`crates/core/src/audit/jsonl.rs`), so manifest timestamps and audit
/// `at` values are formatted identically — and we pull in no date/time crate.
fn now_iso8601_utc() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let ((y, mo, d), (h, mi, s)) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

/// Decompose epoch-seconds into `((year, month, day), (h, m, s))` UTC.
/// Howard Hinnant's `civil_from_days`; mirrors
/// `crates/core/src/audit/jsonl.rs::civil_from_unix`.
fn civil_from_unix(secs: i64) -> ((i32, u32, u32), (u32, u32, u32)) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    ((y, mo, d), (h, mi, s))
}

/// Failure modes specific to `concerto backup`. Surfaced via
/// [`CommandError::Backup`]; `main` renders these to stderr.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// The output directory could not be created.
    #[error("creating backup output directory {path}: {source}")]
    CreateOut {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Resolving the canonical DB path failed (e.g. `$HOME` unset).
    #[error(
        "could not resolve the Concerto DB path: {0}. \
         Set $CONCERTO_DB_PATH or $CONCERTO_DATA_DIR (or $CONCERTO_HOME) to point at your data."
    )]
    ResolveDb(String),
    /// The source DB file does not exist at the resolved path.
    #[error(
        "no Concerto database at {path}. \
         Has the Core ever run? Override the location with $CONCERTO_DB_PATH / $CONCERTO_DATA_DIR."
    )]
    SourceMissing { path: PathBuf },
    /// Opening (or closing) the source DB read-only failed.
    #[error("opening the source database at {path}: {source}")]
    OpenSource {
        path: PathBuf,
        #[source]
        source: Box<sqlx::Error>,
    },
    /// The `VACUUM INTO` itself failed.
    #[error("VACUUM INTO {dest} failed: {source}")]
    Vacuum {
        dest: PathBuf,
        #[source]
        source: Box<sqlx::Error>,
    },
    /// A filesystem operation (tar / audit export / manifest) failed.
    #[error("{what}: {source}")]
    Io {
        what: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// A blocking worker thread (tar / audit export) panicked or was cancelled.
    #[error("backup worker task failed: {0}")]
    Join(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_matches_known_epoch() {
        // 2026-05-30T13:45:30 UTC == 1_780_148_730 seconds since epoch
        // (cross-checked with `date -u -d @1780148730`).
        let ((y, mo, d), (h, mi, s)) = civil_from_unix(1_780_148_730);
        assert_eq!((y, mo, d), (2026, 5, 30));
        assert_eq!((h, mi, s), (13, 45, 30));
    }

    #[test]
    fn range_is_inclusive_and_unbounded_on_absent_sides() {
        let at = "2026-05-30T12:00:00.000Z";
        assert!(in_range(at, None, None));
        assert!(in_range(at, Some("2026-05-30"), Some("2026-05-31")));
        assert!(in_range(at, Some("2026-05-30T12:00:00.000Z"), None));
        assert!(in_range(at, None, Some("2026-05-30T12:00:00.000Z")));
        assert!(!in_range(at, Some("2026-05-31"), None));
        assert!(!in_range(at, None, Some("2026-05-29")));
    }

    #[test]
    fn record_at_extracts_the_timestamp() {
        let line = r#"{"at":"2026-05-30T12:00:00.000Z","kind":"workspace_created"}"#;
        assert_eq!(record_at(line).as_deref(), Some("2026-05-30T12:00:00.000Z"));
        assert_eq!(record_at("not json").as_deref(), None);
        assert_eq!(record_at(r#"{"no":"at"}"#).as_deref(), None);
    }
}

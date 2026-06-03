//! gRPC `Files` service handler (Task 203).
//!
//! Split-host file transfer: a client (Desktop in remote mode, mobile)
//! that does NOT share a filesystem with the Core uploads/downloads files
//! over four RPCs — `Upload` (client→Core streaming), `Download`
//! (Core→client streaming), `Stat`, and `List`. Co-located clients don't
//! need this (`design/10 §2`); the real over-Iroh transfer is exercised
//! split-host by Task 220 (Tier 3). This task proves chunking, checksum,
//! and scoping co-located in CI.
//!
//! ## Security: scope + the policy floor
//!
//! Every RPC resolves `(workarea_id, repository_id?, relative_path)` to a
//! single absolute target under a **scope root**:
//!
//! - `repository_id` set ⇒ the per-workarea checkout
//!   (`workarea_repos.worktree_path`, i.e. `<worktree_root>/<repo.name>`).
//! - `repository_id` unset ⇒ the workarea's `.context/` directory.
//!
//! The target is then `classify`-ed against the live
//! [`crate::security::path_policy`] allow-list + hard deny-list
//! ([`for_workarea_from_db`]). **Only [`PathDecision::Allowed`] proceeds**
//! — `Outside` and `Denied` both reject with `PERMISSION_DENIED`. There is
//! no interactive approval ceremony on a streamed RPC; the strict floor is
//! the contract (`design/12 §3.5`/`§3.7`). A `relative_path` that is
//! absolute or contains a `..` component is rejected up front
//! (`INVALID_ARGUMENT`) as a cheap defense; `classify`'s canonicalization
//! is the authoritative symlink-escape defense.
//!
//! No file is opened or created before `classify` returns `Allowed`.
//!
//! ## Upload atomicity
//!
//! Uploads stream into a temp file inside the SAME scope directory (so the
//! final `rename` is same-filesystem and atomic), feed a BLAKE2b-256
//! hasher incrementally (never buffering the whole file — that would blow
//! the 16 MiB payload budget and defeat chunking), then on `finalize`
//! verify the supplied digest and the byte count before renaming temp →
//! target. On any error the temp file is removed.
//!
//! ## Cross-platform
//!
//! Uses only `std::path` / `tokio::fs` — no `std::os::unix`. Builds on the
//! Windows CI lane (Task 113).

use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use concerto_persist::{Persistence, RepositoryId, WorkareaId};
use concerto_proto::v1::files_server::Files as FilesService;
use concerto_proto::v1::upload_chunk::Body as UploadBody;
use concerto_proto::v1::{
    DownloadChunk, DownloadRequest, FileEntry, ListFilesRequest, ListFilesResponse, StatRequest,
    StatResult, UploadChunk, UploadHeader, UploadResult,
};
use futures::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

use crate::security::path_policy::{self, PathDecision};

/// Max bytes per `data` frame (`design/10 §5.1`). Frames larger than this
/// are rejected so a client cannot blow the 16 MiB gRPC payload budget.
const MAX_CHUNK: usize = 256 * 1024;

/// BLAKE2b with a 256-bit (32-byte) output. FROZEN by Task 203's proto
/// comment (`UploadFinalize.blake2b`).
type Blake2b256 = Blake2b<U32>;

/// Implements the generated `Files` service trait.
///
/// Holds an `Arc<Persistence>` to resolve `(workarea, repo)` → scope root
/// via [`path_policy::for_workarea_from_db`], plus the user's `home` dir
/// (so the hard deny-list expands correctly). Cheap to clone.
#[derive(Clone)]
pub struct FilesHandler {
    persistence: Arc<Persistence>,
    home: PathBuf,
}

impl FilesHandler {
    /// Build a handler. `home` is passed in (rather than read here) so
    /// tests can fake it without touching `$HOME`.
    pub fn new(persistence: Arc<Persistence>, home: PathBuf) -> Self {
        Self { persistence, home }
    }

    /// Resolve the scope root for a `(workarea, repository?)` pair.
    ///
    /// - `repository_id` set ⇒ the per-workarea checkout
    ///   (`workarea_repos.worktree_path`).
    /// - `repository_id` unset ⇒ `<worktree_root>/.context`.
    ///
    /// Both roots are, by construction, members of the workarea allow-list
    /// — but the caller still `classify`-es the final target, so this is
    /// scope resolution, not enforcement.
    async fn scope_root(
        &self,
        workarea_id: &str,
        repository_id: Option<&str>,
    ) -> Result<PathBuf, Status> {
        if workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let pool = self.persistence.readers();
        let wa_id = WorkareaId(workarea_id.to_string());
        let workarea = concerto_persist::workareas::get(pool, &wa_id)
            .await
            .map_err(|e| Status::internal(format!("files.scope: {e}")))?
            .ok_or_else(|| Status::not_found(format!("files.workarea_not_found: {workarea_id}")))?;

        match repository_id {
            None => Ok(Path::new(&workarea.worktree_root).join(".context")),
            Some(repo_id) => {
                if repo_id.is_empty() {
                    return Err(Status::invalid_argument(
                        "repository_id, when set, must be non-empty",
                    ));
                }
                let path = concerto_persist::workareas::get_workarea_repo_worktree_path(
                    pool,
                    &wa_id,
                    &RepositoryId(repo_id.to_string()),
                )
                .await
                .map_err(|e| Status::internal(format!("files.scope: {e}")))?
                .ok_or_else(|| {
                    Status::not_found(format!(
                        "files.repo_not_in_workarea: repo {repo_id} not attached to workarea {workarea_id}"
                    ))
                })?;
                Ok(PathBuf::from(path))
            }
        }
    }

    /// Resolve `(workarea, repo?, relative_path)` to an absolute target
    /// and enforce the policy floor. Returns the absolute (uncanonicalized)
    /// path on `Allowed`; rejects otherwise.
    ///
    /// `relative_path` MUST be relative and free of `..` components — a
    /// cheap up-front defense. The authoritative check is `classify`,
    /// which canonicalizes (resolving symlinks) before prefix-matching.
    async fn resolve_allowed(
        &self,
        workarea_id: &str,
        repository_id: Option<&str>,
        relative_path: &str,
    ) -> Result<PathBuf, Status> {
        reject_unsafe_relative_path(relative_path)?;
        let scope_root = self.scope_root(workarea_id, repository_id).await?;
        // Canonicalize the scope root (it exists on disk and is, by
        // construction, an allow-list root) so the joined target shares the
        // canonical prefix the allow-list was built from. Without this, on
        // platforms where the data root lives behind a symlink (macOS:
        // `/var` → `/private/var`), a target whose leaf doesn't exist yet
        // falls back to lexical cleaning and keeps the un-canonical prefix,
        // so `classify` would wrongly report `Outside`. If the scope root
        // itself can't be canonicalized (it was removed), fall back to the
        // raw form — `classify` will then reject as `Outside`, which is the
        // safe direction.
        let scope_root = path_policy::canonicalize_or_clean(&scope_root);
        let target = scope_root.join(relative_path);

        let (allow, deny) = path_policy::for_workarea_from_db(
            &self.persistence,
            &WorkareaId(workarea_id.to_string()),
            &self.home,
        )
        .await
        .map_err(|e| Status::internal(format!("files.policy: {e}")))?;

        match path_policy::classify(&target, &allow, &deny) {
            PathDecision::Allowed => Ok(target),
            PathDecision::Outside => Err(Status::permission_denied(format!(
                "files.outside_scope: {relative_path} resolves outside the (workarea, repo) allow-list"
            ))),
            PathDecision::Denied => Err(Status::permission_denied(format!(
                "files.denied: {relative_path} resolves into a hard deny-list path"
            ))),
        }
    }
}

/// Reject an absolute `relative_path` or one containing a `..` (parent)
/// component. Empty paths are also rejected. Plain `.` and normal
/// components are fine.
///
/// `tonic::Status` is a large error type; the trait-impl RPC methods that
/// return it are exempt from `result_large_err` by the trait signature, but
/// this free helper needs the explicit allow (matching the pattern in
/// `api_server.rs::tag_uds`). The function is on a cold validation path, so
/// the size of the `Err` variant is immaterial.
#[allow(clippy::result_large_err)]
fn reject_unsafe_relative_path(relative_path: &str) -> Result<(), Status> {
    if relative_path.is_empty() {
        return Err(Status::invalid_argument("relative_path is required"));
    }
    let p = Path::new(relative_path);
    for component in p.components() {
        match component {
            Component::ParentDir => {
                return Err(Status::invalid_argument(
                    "files.path_escape: relative_path must not contain '..' components",
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Status::invalid_argument(
                    "files.path_escape: relative_path must be relative, not absolute",
                ));
            }
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

/// Detect a probable text content-type for `Stat`/`List` results when the
/// upload didn't echo one. V0.1 is deliberately minimal — no sniffing
/// beyond a directory check; `content_type` is `""` for files (clients
/// fall back to the extension). Kept as a helper so the surface is stable.
fn empty_content_type() -> String {
    String::new()
}

type DownloadStream = Pin<Box<dyn Stream<Item = Result<DownloadChunk, Status>> + Send + 'static>>;

#[async_trait]
impl FilesService for FilesHandler {
    #[tracing::instrument(skip_all, name = "Files::Upload")]
    async fn upload(
        &self,
        request: Request<Streaming<UploadChunk>>,
    ) -> Result<Response<UploadResult>, Status> {
        let mut stream = request.into_inner();

        // 1. First frame MUST be the header.
        let header = match stream.next().await {
            Some(Ok(chunk)) => match chunk.body {
                Some(UploadBody::Header(h)) => h,
                Some(_) => {
                    return Err(Status::invalid_argument(
                        "files.upload: first frame must be UploadHeader",
                    ));
                }
                None => {
                    return Err(Status::invalid_argument(
                        "files.upload: first frame has empty body",
                    ));
                }
            },
            Some(Err(s)) => return Err(s),
            None => {
                return Err(Status::invalid_argument(
                    "files.upload: stream closed before header",
                ));
            }
        };

        let UploadHeader {
            workarea_id,
            repository_id,
            relative_path,
            expected_size,
            content_type: _content_type,
        } = header;

        // 2. Resolve scope + enforce the policy floor BEFORE opening any
        //    file.
        let target = self
            .resolve_allowed(&workarea_id, repository_id.as_deref(), &relative_path)
            .await?;
        let scope_dir = target.parent().ok_or_else(|| {
            Status::invalid_argument("files.upload: target has no parent directory")
        })?;

        // 3. Ensure the parent directory exists (within scope), then open a
        //    temp file in the SAME directory so the final rename is atomic.
        tokio::fs::create_dir_all(scope_dir)
            .await
            .map_err(|e| Status::internal(format!("files.upload.mkdir: {e}")))?;

        let temp_path = temp_path_for(&target);
        let mut file = tokio::fs::File::create(&temp_path)
            .await
            .map_err(|e| Status::internal(format!("files.upload.create_temp: {e}")))?;

        // 4. Stream `data` frames into the temp file, hashing as we go.
        //    `finalize` ends the stream. On any error, remove the temp.
        let mut hasher = Blake2b256::new();
        let mut written: u64 = 0;
        let mut finalize_digest: Option<Vec<u8>> = None;

        let result: Result<(), Status> = async {
            while let Some(item) = stream.next().await {
                let chunk = item?;
                match chunk.body {
                    Some(UploadBody::Data(data)) => {
                        if data.len() > MAX_CHUNK {
                            return Err(Status::invalid_argument(format!(
                                "files.upload.oversize_chunk: {} bytes exceeds the {MAX_CHUNK}-byte limit",
                                data.len()
                            )));
                        }
                        hasher.update(&data);
                        file.write_all(&data)
                            .await
                            .map_err(|e| Status::internal(format!("files.upload.write: {e}")))?;
                        written += data.len() as u64;
                    }
                    Some(UploadBody::Finalize(fin)) => {
                        finalize_digest = Some(fin.blake2b);
                        break;
                    }
                    Some(UploadBody::Header(_)) => {
                        return Err(Status::invalid_argument(
                            "files.upload: unexpected second UploadHeader",
                        ));
                    }
                    None => {
                        return Err(Status::invalid_argument(
                            "files.upload: frame with empty body",
                        ));
                    }
                }
            }
            file.flush()
                .await
                .map_err(|e| Status::internal(format!("files.upload.flush: {e}")))?;
            file.sync_all()
                .await
                .map_err(|e| Status::internal(format!("files.upload.sync: {e}")))?;
            Ok(())
        }
        .await;

        if let Err(e) = result {
            remove_temp_best_effort(&temp_path).await;
            return Err(e);
        }

        // 5. A finalize frame is mandatory — without it we cannot verify
        //    integrity.
        let supplied = match finalize_digest {
            Some(d) => d,
            None => {
                remove_temp_best_effort(&temp_path).await;
                return Err(Status::invalid_argument(
                    "files.upload: stream ended without an UploadFinalize frame",
                ));
            }
        };

        // 6. Verify checksum + size.
        let computed = hasher.finalize();
        if supplied.as_slice() != computed.as_slice() {
            remove_temp_best_effort(&temp_path).await;
            return Err(Status::new(
                tonic::Code::DataLoss,
                "files.checksum_mismatch: computed BLAKE2b-256 does not match the supplied digest",
            ));
        }
        if written != expected_size {
            remove_temp_best_effort(&temp_path).await;
            return Err(Status::invalid_argument(format!(
                "files.size_mismatch: received {written} bytes, header declared {expected_size}"
            )));
        }

        // 7. Atomic rename temp → target.
        if let Err(e) = tokio::fs::rename(&temp_path, &target).await {
            remove_temp_best_effort(&temp_path).await;
            return Err(Status::internal(format!("files.upload.rename: {e}")));
        }

        Ok(Response::new(UploadResult {
            stored_path: target.to_string_lossy().into_owned(),
            size: written,
        }))
    }

    type DownloadStream = DownloadStream;

    #[tracing::instrument(skip_all, name = "Files::Download")]
    async fn download(
        &self,
        request: Request<DownloadRequest>,
    ) -> Result<Response<Self::DownloadStream>, Status> {
        let DownloadRequest {
            workarea_id,
            repository_id,
            relative_path,
            offset,
            length,
        } = request.into_inner();

        let target = self
            .resolve_allowed(&workarea_id, repository_id.as_deref(), &relative_path)
            .await?;

        let mut file = tokio::fs::File::open(&target)
            .await
            .map_err(|e| Status::not_found(format!("files.download.open: {e}")))?;

        let metadata = file
            .metadata()
            .await
            .map_err(|e| Status::internal(format!("files.download.metadata: {e}")))?;
        if metadata.is_dir() {
            return Err(Status::invalid_argument(
                "files.download: target is a directory",
            ));
        }
        let file_len = metadata.len();

        // Resolve the byte range. `offset` past EOF yields an empty stream;
        // `length` clamps to the bytes remaining after `offset`.
        let start = offset.unwrap_or(0);
        if start > 0 {
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| Status::internal(format!("files.download.seek: {e}")))?;
        }
        let remaining_from_offset = file_len.saturating_sub(start);
        let mut remaining = match length {
            Some(l) => l.min(remaining_from_offset),
            None => remaining_from_offset,
        };

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<DownloadChunk, Status>>(8);
        tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_CHUNK];
            while remaining > 0 {
                let want = (remaining as usize).min(MAX_CHUNK);
                match file.read(&mut buf[..want]).await {
                    Ok(0) => break, // EOF (file truncated underneath us)
                    Ok(n) => {
                        remaining -= n as u64;
                        if tx
                            .send(Ok(DownloadChunk {
                                data: buf[..n].to_vec(),
                            }))
                            .await
                            .is_err()
                        {
                            // Client dropped the stream.
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(Status::internal(format!("files.download.read: {e}"))))
                            .await;
                        return;
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    #[tracing::instrument(skip_all, name = "Files::Stat")]
    async fn stat(&self, request: Request<StatRequest>) -> Result<Response<StatResult>, Status> {
        let StatRequest {
            workarea_id,
            repository_id,
            relative_path,
        } = request.into_inner();

        let target = self
            .resolve_allowed(&workarea_id, repository_id.as_deref(), &relative_path)
            .await?;

        match tokio::fs::metadata(&target).await {
            Ok(md) => Ok(Response::new(StatResult {
                exists: true,
                size: md.len(),
                is_dir: md.is_dir(),
                content_type: empty_content_type(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Response::new(StatResult {
                exists: false,
                size: 0,
                is_dir: false,
                content_type: empty_content_type(),
            })),
            Err(e) => Err(Status::internal(format!("files.stat: {e}"))),
        }
    }

    #[tracing::instrument(skip_all, name = "Files::List")]
    async fn list(
        &self,
        request: Request<ListFilesRequest>,
    ) -> Result<Response<ListFilesResponse>, Status> {
        let ListFilesRequest {
            workarea_id,
            repository_id,
            relative_path,
        } = request.into_inner();

        let target = self
            .resolve_allowed(&workarea_id, repository_id.as_deref(), &relative_path)
            .await?;

        let md = tokio::fs::metadata(&target)
            .await
            .map_err(|e| Status::not_found(format!("files.list.stat: {e}")))?;
        if !md.is_dir() {
            return Err(Status::invalid_argument(
                "files.list: target is not a directory",
            ));
        }

        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(&target)
            .await
            .map_err(|e| Status::internal(format!("files.list.read_dir: {e}")))?;
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| Status::internal(format!("files.list.next: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_md = entry
                .metadata()
                .await
                .map_err(|e| Status::internal(format!("files.list.entry_meta: {e}")))?;
            entries.push(FileEntry {
                name,
                size: entry_md.len(),
                is_dir: entry_md.is_dir(),
            });
        }
        // Stable, deterministic ordering by name.
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(Response::new(ListFilesResponse { entries }))
    }
}

/// Build a sibling temp path for an upload target. The temp lives in the
/// SAME directory as the target so the final rename is same-filesystem
/// (atomic). A random-ish suffix avoids collisions between concurrent
/// uploads to the same path.
fn temp_path_for(target: &Path) -> PathBuf {
    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "upload".to_string());
    // Nanosecond clock + the OS pid give enough uniqueness for a temp
    // sibling without pulling in an RNG crate.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let suffix = format!(".{}.{}.tmp", std::process::id(), nanos);
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(".{file_name}{suffix}"))
}

/// Remove a temp file, ignoring errors (the path may already be gone if
/// the rename succeeded before an error on a later step).
async fn remove_temp_best_effort(path: &Path) {
    if let Err(e) = tokio::fs::remove_file(path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(temp = %path.display(), error = %e, "files: failed to remove upload temp");
        }
    }
}

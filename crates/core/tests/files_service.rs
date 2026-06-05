//! Integration tests for Task 203 — the `Files` gRPC service
//! (`crates/core/src/handlers/files.rs`).
//!
//! Each test stands up a real in-process tonic server hosting ONLY the
//! `FilesServer`, over a Unix Domain Socket, backed by a real
//! `Persistence` whose `(project → repo → workspace → workarea →
//! workarea_repos)` chain is seeded against on-disk worktree dirs (so the
//! `path_policy` canonicalization resolves the same way the handler does).
//! A `FilesClient` dialed over the socket drives the streaming RPCs end to
//! end. The handler is fed a fake `home` so the hard deny-list expands
//! under a tempdir we control.
//!
//! Coverage (per the task's Scope — in test list):
//! - round-trip Upload → Download of a multi-chunk file, matching checksum;
//! - a tampered checksum on finalize → reject (`DataLoss`);
//! - a `relative_path` with `..` → reject (`InvalidArgument`);
//! - an Upload targeting an outside-scope path → `PermissionDenied`;
//! - an Upload into a hard deny-list path → `PermissionDenied`;
//! - `expected_size` mismatch → reject;
//! - a `data` frame > 256 KiB → reject;
//! - `Download` with offset/length returns the right slice;
//! - `Stat` / `List` on an in-scope path.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use concerto_core::handlers::files::FilesHandler;
use concerto_persist::{
    NewProject, NewRepository, NewWorkarea, NewWorkareaRepo, NewWorkspace, Persistence,
    PersistenceConfig, ProjectId, RepositoryId, WorkareaId, WorkspaceId,
};
use concerto_proto::v1::files_client::FilesClient;
use concerto_proto::v1::files_server::FilesServer;
use concerto_proto::v1::upload_chunk::Body as UploadBody;
use concerto_proto::v1::{
    DownloadRequest, ListFilesRequest, StatRequest, UploadChunk, UploadFinalize, UploadHeader,
};
use futures::StreamExt;
use hyper_util::rt::TokioIo;
use tempfile::TempDir;
use tokio::net::{UnixListener, UnixStream};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::{Channel, Endpoint, Server, Uri};
use tonic::Code;

type Blake2b256 = Blake2b<U32>;

/// The fixture owns the tempdirs (kept alive for the test) and exposes the
/// seeded ids + the live `FilesClient`.
struct Fixture {
    client: FilesClient<Channel>,
    workarea_id: String,
    repo_id: String,
    /// `<home>` — the deny-list expands under here.
    home: PathBuf,
    /// The per-workarea repo checkout dir on disk (scope root when
    /// `repository_id` is set).
    repo_worktree: PathBuf,
    /// The workarea `.context/` dir (scope root when `repository_id` unset).
    context_dir: PathBuf,
    _tmp: TempDir,
}

async fn setup() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Fake home (deny-list root). The Core data root `<home>/concerto` is
    // also part of the allow-list; we keep the worktree under it so the
    // layout matches production.
    let home = root.join("home");
    let concerto_root = home.join("concerto");
    let worktree_root = concerto_root.join("workspaces/ws/bach");
    let repo_worktree = worktree_root.join("repo");
    let context_dir = worktree_root.join(".context");
    let repo_local_path = concerto_root.join("repos/repo");

    for d in [&repo_worktree, &context_dir, &repo_local_path] {
        tokio::fs::create_dir_all(d).await.unwrap();
    }

    // Persistence + the FK chain.
    let data = root.join("data");
    tokio::fs::create_dir_all(&data).await.unwrap();
    let persist = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path: data.join("concerto.db"),
            max_readers: 2,
        })
        .await
        .expect("open persistence"),
    );

    let project_id = ProjectId(format!("proj-{}", uuid::Uuid::now_v7()));
    let repo_id = RepositoryId(format!("repo-{}", uuid::Uuid::now_v7()));
    let workspace_id = WorkspaceId(format!("ws-{}", uuid::Uuid::now_v7()));
    let workarea_id = WorkareaId(format!("wa-{}", uuid::Uuid::now_v7()));

    {
        let mut writer = persist.writer().await;
        concerto_persist::projects::insert(
            &mut writer,
            NewProject {
                id: project_id.clone(),
                name: "files-test".into(),
                icon: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        concerto_persist::repositories::insert(
            &mut writer,
            NewRepository {
                id: repo_id.clone(),
                project_id: project_id.0.clone(),
                name: "repo".into(),
                url: "https://github.com/owner/repo".into(),
                local_path: repo_local_path.to_string_lossy().into_owned(),
                clone_strategy: "full".into(),
                default_branch: "main".into(),
            },
        )
        .await
        .unwrap();
        concerto_persist::workspaces::insert(
            &mut writer,
            NewWorkspace {
                id: workspace_id.clone(),
                project_id: project_id.0.clone(),
                name: "ws".into(),
                slug: "ws".into(),
                description: None,
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        concerto_persist::workspaces::update_repos(
            &mut writer,
            &workspace_id,
            std::slice::from_ref(&repo_id),
        )
        .await
        .unwrap();
        concerto_persist::workareas::insert(
            &mut writer,
            NewWorkarea {
                id: workarea_id.clone(),
                workspace_id: workspace_id.0.clone(),
                composer_name: "bach".into(),
                branch_name: "concerto/bach".into(),
                worktree_root: worktree_root.to_string_lossy().into_owned(),
                status: "active".into(),
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        concerto_persist::workareas::insert_workarea_repo(
            &mut writer,
            NewWorkareaRepo {
                workarea_id: workarea_id.clone(),
                repository_id: repo_id.clone(),
                worktree_path: repo_worktree.to_string_lossy().into_owned(),
                branch_override: None,
                // Task 302: default-empty cone set.
                sparse_cones_json: NewWorkareaRepo::empty_cones(),
            },
        )
        .await
        .unwrap();
    }

    // Serve FilesServer over a UDS.
    let socket = root.join("files.sock");
    let handler = FilesHandler::new(persist.clone(), home.clone());
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        Server::builder()
            .add_service(FilesServer::new(handler))
            .serve_with_incoming(UnixListenerStream::new(listener))
            .await
            .ok();
    });

    let client = FilesClient::new(uds_channel(socket).await);

    Fixture {
        client,
        workarea_id: workarea_id.0,
        repo_id: repo_id.0,
        home,
        repo_worktree,
        context_dir,
        _tmp: tmp,
    }
}

async fn uds_channel(socket: PathBuf) -> Channel {
    Endpoint::try_from("http://[::1]:50051")
        .unwrap()
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = socket.clone();
            async move { Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(&p).await?)) }
        }))
        .await
        .expect("connect uds")
}

fn blake2b_256(bytes: &[u8]) -> Vec<u8> {
    let mut h = Blake2b256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// Build an upload frame stream: header, N data frames, finalize.
fn upload_frames(
    workarea_id: &str,
    repository_id: Option<&str>,
    relative_path: &str,
    payload: &[u8],
    chunk: usize,
    blake2b: Vec<u8>,
    expected_size: u64,
) -> Vec<UploadChunk> {
    let mut frames = vec![UploadChunk {
        body: Some(UploadBody::Header(UploadHeader {
            workarea_id: workarea_id.to_string(),
            repository_id: repository_id.map(|s| s.to_string()),
            relative_path: relative_path.to_string(),
            expected_size,
            content_type: "application/octet-stream".to_string(),
        })),
    }];
    for piece in payload.chunks(chunk.max(1)) {
        frames.push(UploadChunk {
            body: Some(UploadBody::Data(piece.to_vec())),
        });
    }
    frames.push(UploadChunk {
        body: Some(UploadBody::Finalize(UploadFinalize { blake2b })),
    });
    frames
}

async fn download_all(
    client: &mut FilesClient<Channel>,
    workarea_id: &str,
    repository_id: Option<&str>,
    relative_path: &str,
    offset: Option<u64>,
    length: Option<u64>,
) -> Result<Vec<u8>, tonic::Status> {
    let resp = client
        .download(DownloadRequest {
            workarea_id: workarea_id.to_string(),
            repository_id: repository_id.map(|s| s.to_string()),
            relative_path: relative_path.to_string(),
            offset,
            length,
        })
        .await?;
    let mut stream = resp.into_inner();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk?.data);
    }
    Ok(out)
}

#[tokio::test(flavor = "multi_thread")]
async fn round_trip_multichunk() {
    let mut fx = setup().await;
    // ~600 KiB so it spans multiple 256 KiB frames on download too.
    let payload: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
    let digest = blake2b_256(&payload);

    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "sub/data.bin",
        &payload,
        200 * 1024,
        digest,
        payload.len() as u64,
    );
    let result = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect("upload ok")
        .into_inner();
    assert_eq!(result.size, payload.len() as u64);
    // The stored file actually exists under the repo worktree.
    assert!(fx.repo_worktree.join("sub/data.bin").exists());
    // The handler returns the canonical stored path (the scope root is
    // canonicalized before joining the relative path), so compare against
    // the canonicalized target.
    assert_eq!(
        result.stored_path,
        std::fs::canonicalize(fx.repo_worktree.join("sub/data.bin"))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    );

    let got = download_all(
        &mut fx.client,
        &fx.workarea_id,
        Some(&fx.repo_id),
        "sub/data.bin",
        None,
        None,
    )
    .await
    .expect("download ok");
    assert_eq!(got, payload, "round-trip bytes differ");
    assert_eq!(blake2b_256(&got), blake2b_256(&payload));
}

#[tokio::test(flavor = "multi_thread")]
async fn context_root_when_repository_unset() {
    let mut fx = setup().await;
    let payload = b"context scratch".to_vec();
    let digest = blake2b_256(&payload);
    let frames = upload_frames(
        &fx.workarea_id,
        None,
        "notes.txt",
        &payload,
        64 * 1024,
        digest,
        payload.len() as u64,
    );
    fx.client
        .upload(futures::stream::iter(frames))
        .await
        .expect("upload to .context ok");
    assert!(fx.context_dir.join("notes.txt").exists());

    let got = download_all(
        &mut fx.client,
        &fx.workarea_id,
        None,
        "notes.txt",
        None,
        None,
    )
    .await
    .expect("download ok");
    assert_eq!(got, payload);
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_checksum_rejected() {
    let mut fx = setup().await;
    let payload = b"hello world".to_vec();
    let mut bad = blake2b_256(&payload);
    bad[0] ^= 0xff; // corrupt the digest
    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "tamper.txt",
        &payload,
        64 * 1024,
        bad,
        payload.len() as u64,
    );
    let err = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect_err("tampered checksum must reject");
    assert_eq!(err.code(), Code::DataLoss, "msg: {}", err.message());
    // The temp file must not have been left behind nor the target created.
    assert!(!fx.repo_worktree.join("tamper.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn path_escape_rejected() {
    let mut fx = setup().await;
    let payload = b"x".to_vec();
    let digest = blake2b_256(&payload);
    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "../escape.txt",
        &payload,
        1024,
        digest,
        payload.len() as u64,
    );
    let err = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect_err("path escape must reject");
    assert_eq!(err.code(), Code::InvalidArgument, "msg: {}", err.message());
}

#[tokio::test(flavor = "multi_thread")]
async fn absolute_path_rejected() {
    let mut fx = setup().await;
    let payload = b"x".to_vec();
    let digest = blake2b_256(&payload);
    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "/etc/passwd",
        &payload,
        1024,
        digest,
        payload.len() as u64,
    );
    let err = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect_err("absolute path must reject");
    assert_eq!(err.code(), Code::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_list_symlink_escape_rejected() {
    // A symlink inside the repo worktree pointing at <home>/.ssh resolves
    // (via classify's canonicalization) into the hard deny-list → reject.
    let mut fx = setup().await;
    let ssh = fx.home.join(".ssh");
    tokio::fs::create_dir_all(&ssh).await.unwrap();
    tokio::fs::write(ssh.join("id_rsa"), b"secret")
        .await
        .unwrap();
    std::os::unix::fs::symlink(&ssh, fx.repo_worktree.join("sneaky")).unwrap();

    let payload = b"pwn".to_vec();
    let digest = blake2b_256(&payload);
    // Target lands under the symlink → canonicalizes into <home>/.ssh.
    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "sneaky/id_rsa",
        &payload,
        1024,
        digest,
        payload.len() as u64,
    );
    let err = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect_err("deny-list path must reject");
    assert_eq!(err.code(), Code::PermissionDenied, "msg: {}", err.message());
}

#[tokio::test(flavor = "multi_thread")]
async fn size_mismatch_rejected() {
    let mut fx = setup().await;
    let payload = b"twelve bytes".to_vec();
    let digest = blake2b_256(&payload);
    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "size.txt",
        &payload,
        1024,
        digest,
        payload.len() as u64 + 5, // lie about the size
    );
    let err = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect_err("size mismatch must reject");
    assert_eq!(err.code(), Code::InvalidArgument, "msg: {}", err.message());
    assert!(!fx.repo_worktree.join("size.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn oversize_chunk_rejected() {
    let mut fx = setup().await;
    // A single 300 KiB data frame exceeds the 256 KiB cap.
    let payload: Vec<u8> = vec![7u8; 300 * 1024];
    let digest = blake2b_256(&payload);
    let frames = vec![
        UploadChunk {
            body: Some(UploadBody::Header(UploadHeader {
                workarea_id: fx.workarea_id.clone(),
                repository_id: Some(fx.repo_id.clone()),
                relative_path: "big.bin".to_string(),
                expected_size: payload.len() as u64,
                content_type: String::new(),
            })),
        },
        UploadChunk {
            body: Some(UploadBody::Data(payload)),
        },
        UploadChunk {
            body: Some(UploadBody::Finalize(UploadFinalize { blake2b: digest })),
        },
    ];
    let err = fx
        .client
        .upload(futures::stream::iter(frames))
        .await
        .expect_err("oversize chunk must reject");
    assert_eq!(err.code(), Code::InvalidArgument, "msg: {}", err.message());
}

#[tokio::test(flavor = "multi_thread")]
async fn download_offset_length_slice() {
    let mut fx = setup().await;
    let payload: Vec<u8> = (0..1000u32).map(|i| (i % 256) as u8).collect();
    let digest = blake2b_256(&payload);
    let frames = upload_frames(
        &fx.workarea_id,
        Some(&fx.repo_id),
        "slice.bin",
        &payload,
        256 * 1024,
        digest,
        payload.len() as u64,
    );
    fx.client
        .upload(futures::stream::iter(frames))
        .await
        .expect("upload ok");

    // offset 100, length 50 → bytes [100, 150).
    let got = download_all(
        &mut fx.client,
        &fx.workarea_id,
        Some(&fx.repo_id),
        "slice.bin",
        Some(100),
        Some(50),
    )
    .await
    .expect("ranged download ok");
    assert_eq!(got, payload[100..150]);

    // offset past EOF → empty.
    let empty = download_all(
        &mut fx.client,
        &fx.workarea_id,
        Some(&fx.repo_id),
        "slice.bin",
        Some(10_000),
        None,
    )
    .await
    .expect("past-eof download ok");
    assert!(empty.is_empty());

    // length past EOF clamps to the remaining bytes.
    let clamped = download_all(
        &mut fx.client,
        &fx.workarea_id,
        Some(&fx.repo_id),
        "slice.bin",
        Some(990),
        Some(1000),
    )
    .await
    .expect("clamped download ok");
    assert_eq!(clamped, payload[990..]);
}

#[tokio::test(flavor = "multi_thread")]
async fn stat_and_list_in_scope() {
    let mut fx = setup().await;
    // Upload two files into a subdir.
    for name in ["a.txt", "b.txt"] {
        let payload = format!("content of {name}").into_bytes();
        let digest = blake2b_256(&payload);
        let frames = upload_frames(
            &fx.workarea_id,
            Some(&fx.repo_id),
            &format!("dir/{name}"),
            &payload,
            1024,
            digest,
            payload.len() as u64,
        );
        fx.client
            .upload(futures::stream::iter(frames))
            .await
            .expect("upload ok");
    }

    // Stat a file.
    let stat = fx
        .client
        .stat(StatRequest {
            workarea_id: fx.workarea_id.clone(),
            repository_id: Some(fx.repo_id.clone()),
            relative_path: "dir/a.txt".to_string(),
        })
        .await
        .expect("stat ok")
        .into_inner();
    assert!(stat.exists);
    assert!(!stat.is_dir);
    assert_eq!(stat.size, "content of a.txt".len() as u64);

    // Stat a directory.
    let stat_dir = fx
        .client
        .stat(StatRequest {
            workarea_id: fx.workarea_id.clone(),
            repository_id: Some(fx.repo_id.clone()),
            relative_path: "dir".to_string(),
        })
        .await
        .expect("stat dir ok")
        .into_inner();
    assert!(stat_dir.exists);
    assert!(stat_dir.is_dir);

    // Stat a missing path → exists=false (no error).
    let missing = fx
        .client
        .stat(StatRequest {
            workarea_id: fx.workarea_id.clone(),
            repository_id: Some(fx.repo_id.clone()),
            relative_path: "dir/missing.txt".to_string(),
        })
        .await
        .expect("stat missing ok")
        .into_inner();
    assert!(!missing.exists);

    // List the dir.
    let list = fx
        .client
        .list(ListFilesRequest {
            workarea_id: fx.workarea_id.clone(),
            repository_id: Some(fx.repo_id.clone()),
            relative_path: "dir".to_string(),
        })
        .await
        .expect("list ok")
        .into_inner();
    let names: Vec<String> = list.entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn download_missing_file_not_found() {
    let mut fx = setup().await;
    let err = download_all(
        &mut fx.client,
        &fx.workarea_id,
        Some(&fx.repo_id),
        "nope.txt",
        None,
        None,
    )
    .await
    .expect_err("missing download must error");
    assert_eq!(err.code(), Code::NotFound, "msg: {}", err.message());
}

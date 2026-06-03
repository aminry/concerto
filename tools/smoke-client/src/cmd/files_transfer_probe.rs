//! `smoke-client files-transfer-probe --workarea-id <wa>` — end-to-end
//! probe of the Task 203 `Files` service over the live UDS Core.
//!
//! Self-contained and deterministic. It targets the workarea's `.context/`
//! root (`repository_id` unset), which is ALWAYS part of the allow-list, so
//! the probe never depends on a repo checkout existing:
//!
//! 1. Upload a small multi-chunk file into `.context/` (chunked ≤ 256 KiB,
//!    incremental BLAKE2b-256, finalize with the digest).
//! 2. Download it back and assert byte-identical + matching BLAKE2b.
//! 3. `Stat` it and assert `exists=true`, the right size, `is_dir=false`.
//! 4. Assert an out-of-scope path (`../escape.txt`) is REJECTED — the
//!    policy floor is enforced before any byte touches disk.
//!
//! Exits 0 on success; on any mismatch prints the discrepancy to stderr
//! and exits 1 (surfaced by the smoke gate).

use std::path::Path;

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use concerto_proto::v1::files_client::FilesClient;
use concerto_proto::v1::upload_chunk::Body as UploadBody;
use concerto_proto::v1::{DownloadRequest, StatRequest, UploadChunk, UploadFinalize, UploadHeader};
use futures::StreamExt;
use tonic::Code;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

type Blake2b256 = Blake2b<U32>;

const REL_PATH: &str = "smoke-files-transfer.bin";
const CHUNK: usize = 200 * 1024;

fn blake2b_256(bytes: &[u8]) -> Vec<u8> {
    let mut h = Blake2b256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

pub async fn run(socket: &Path, workarea_id: &str) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut files = FilesClient::new(channel);

    // A ~450 KiB payload so the upload spans multiple 256 KiB frames.
    let payload: Vec<u8> = (0..450 * 1024).map(|i| (i % 251) as u8).collect();
    let digest = blake2b_256(&payload);

    // 1. Upload into the workarea's .context/ (repository_id unset).
    let mut frames = vec![UploadChunk {
        body: Some(UploadBody::Header(UploadHeader {
            workarea_id: workarea_id.to_string(),
            repository_id: None,
            relative_path: REL_PATH.to_string(),
            expected_size: payload.len() as u64,
            content_type: "application/octet-stream".to_string(),
        })),
    }];
    for piece in payload.chunks(CHUNK) {
        frames.push(UploadChunk {
            body: Some(UploadBody::Data(piece.to_vec())),
        });
    }
    frames.push(UploadChunk {
        body: Some(UploadBody::Finalize(UploadFinalize {
            blake2b: digest.clone(),
        })),
    });

    let upload_result =
        tokio::time::timeout(RPC_TIMEOUT, files.upload(futures::stream::iter(frames)))
            .await
            .map_err(|_| format!("Upload timed out after {RPC_TIMEOUT:?}"))?
            .map_err(|s| format!("Upload rpc error: {s}"))?
            .into_inner();
    if upload_result.size != payload.len() as u64 {
        return Err(format!(
            "Upload reported size {} but payload was {} bytes",
            upload_result.size,
            payload.len()
        ));
    }

    // 2. Download it back; assert byte-identical + checksum.
    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        files.download(DownloadRequest {
            workarea_id: workarea_id.to_string(),
            repository_id: None,
            relative_path: REL_PATH.to_string(),
            offset: None,
            length: None,
        }),
    )
    .await
    .map_err(|_| format!("Download timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("Download rpc error: {s}"))?;
    let mut stream = resp.into_inner();
    let mut downloaded = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|s| format!("Download stream error: {s}"))?;
        downloaded.extend_from_slice(&chunk.data);
    }
    if downloaded != payload {
        return Err(format!(
            "round-trip mismatch: downloaded {} bytes, expected {}",
            downloaded.len(),
            payload.len()
        ));
    }
    if blake2b_256(&downloaded) != digest {
        return Err("round-trip BLAKE2b-256 mismatch".to_string());
    }

    // 3. Stat it.
    let stat = tokio::time::timeout(
        RPC_TIMEOUT,
        files.stat(StatRequest {
            workarea_id: workarea_id.to_string(),
            repository_id: None,
            relative_path: REL_PATH.to_string(),
        }),
    )
    .await
    .map_err(|_| format!("Stat timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("Stat rpc error: {s}"))?
    .into_inner();
    if !stat.exists || stat.is_dir || stat.size != payload.len() as u64 {
        return Err(format!(
            "Stat unexpected: exists={} is_dir={} size={} (want exists=true is_dir=false size={})",
            stat.exists,
            stat.is_dir,
            stat.size,
            payload.len()
        ));
    }

    // 4. An out-of-scope path must be rejected with PERMISSION_DENIED
    //    (or INVALID_ARGUMENT for the cheap path-escape pre-check). Either
    //    is an acceptable rejection of the escape; what matters is it does
    //    NOT succeed.
    let escape_payload = b"nope".to_vec();
    let escape_digest = blake2b_256(&escape_payload);
    let escape_frames = vec![
        UploadChunk {
            body: Some(UploadBody::Header(UploadHeader {
                workarea_id: workarea_id.to_string(),
                repository_id: None,
                relative_path: "../escape.txt".to_string(),
                expected_size: escape_payload.len() as u64,
                content_type: String::new(),
            })),
        },
        UploadChunk {
            body: Some(UploadBody::Data(escape_payload)),
        },
        UploadChunk {
            body: Some(UploadBody::Finalize(UploadFinalize {
                blake2b: escape_digest,
            })),
        },
    ];
    match tokio::time::timeout(
        RPC_TIMEOUT,
        files.upload(futures::stream::iter(escape_frames)),
    )
    .await
    .map_err(|_| format!("escape Upload timed out after {RPC_TIMEOUT:?}"))?
    {
        Ok(_) => {
            return Err(
                "out-of-scope '../escape.txt' upload SUCCEEDED — policy floor not enforced"
                    .to_string(),
            );
        }
        Err(status) => match status.code() {
            Code::PermissionDenied | Code::InvalidArgument => {}
            other => {
                return Err(format!(
                    "out-of-scope upload rejected with unexpected code {other:?}: {status}"
                ));
            }
        },
    }

    println!(
        "files-transfer-probe: OK (round-trip + checksum + stat + out-of-scope reject verified)"
    );
    Ok(())
}

//! Final-info writer.
//!
//! When the PTY child exits the host writes a small JSON document to
//! `--final-info` so a late-arriving Core (e.g. one that started after
//! the host already died) can still surface a meaningful "agent ended"
//! event. Schema is locked by Task 21's spec; see [`crate::api::FinalInfo`].
//!
//! The write is best-effort: we log on failure but do not propagate the
//! error to the connection loop because by that point the agent is gone
//! and there is no recovery action.

use std::path::Path;

use tokio::fs;

use crate::api::FinalInfo;

/// Serialize `info` to JSON and write it atomically-ish to `path`.
///
/// "Atomically-ish" means we write through a `.tmp` sibling and rename
/// into place — good enough for a single-writer, single-reader handoff
/// on the same filesystem. If the rename fails we fall back to a direct
/// write so a malformed temp path doesn't lose the data entirely.
pub async fn write_final_info(path: &Path, info: &FinalInfo) -> std::io::Result<()> {
    let payload = serde_json::to_vec_pretty(info)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).await.ok();
        }
    }
    let tmp = path.with_extension("final.json.tmp");
    match fs::write(&tmp, &payload).await {
        Ok(()) => match fs::rename(&tmp, path).await {
            Ok(()) => Ok(()),
            Err(_) => fs::write(path, &payload).await,
        },
        Err(_) => fs::write(path, &payload).await,
    }
}

/// Extract the last `n` lines from the bytes the host saw. Used by the
/// connection loop to populate [`FinalInfo::last_lines`] without keeping
/// a parallel line-buffered copy of stdout.
pub fn tail_lines(buf: &[u8], n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(buf);
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    if lines.len() > n {
        let drop = lines.len() - n;
        lines.drain(..drop);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn writes_and_reads_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("final.json");
        let info = FinalInfo {
            exit_code: Some(0),
            signal: None,
            last_lines: vec!["hello".into()],
            external_session_id: Some("sess-1".into()),
            exited_at_unix_ms: 1_716_800_000_123,
        };
        write_final_info(&path, &info).await.unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: FinalInfo = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.exit_code, Some(0));
        assert_eq!(parsed.last_lines, vec!["hello".to_string()]);
        assert_eq!(parsed.external_session_id.as_deref(), Some("sess-1"));
        assert_eq!(parsed.exited_at_unix_ms, 1_716_800_000_123);
    }

    #[test]
    fn tail_returns_last_n_lines() {
        let buf = b"a\nb\nc\nd\ne\n";
        let got = tail_lines(buf, 3);
        assert_eq!(got, vec!["c".to_string(), "d".into(), "e".into()]);
    }

    #[test]
    fn tail_handles_short_input() {
        let buf = b"only-one";
        let got = tail_lines(buf, 100);
        assert_eq!(got, vec!["only-one".to_string()]);
    }
}

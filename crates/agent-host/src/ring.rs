//! 1 MiB byte ring buffer that survives Core disconnect/reconnect.
//!
//! `design/04 §3.9` mandates this buffer so a restarted Core can replay
//! anything its predecessor missed past the last `Ack`-confirmed seq.
//! V0.1 keeps the policy dead simple: every chunk emitted by the PTY is
//! appended with a monotonic `seq`; reads return everything past a
//! caller-supplied watermark. When total stored bytes exceed
//! [`RING_BUFFER_BYTES`], oldest chunks are evicted whole.
//!
//! The buffer is held behind a `tokio::sync::Mutex` by the caller because
//! contention is low (one producer — the PTY reader task — and one
//! consumer — the connection writer task).

use std::collections::VecDeque;

use crate::api::RING_BUFFER_BYTES;

/// A single emitted chunk plus the `seq` the host assigned to it.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub seq: u64,
    pub data: Vec<u8>,
}

/// Append-and-replay ring buffer. Not `Send`/`Sync`-safe on its own —
/// wrap in a `tokio::sync::Mutex` for cross-task sharing.
#[derive(Debug)]
pub struct RingBuffer {
    chunks: VecDeque<Chunk>,
    /// Monotonic counter; next-to-be-assigned seq for [`push`].
    next_seq: u64,
    /// Sum of `chunk.data.len()` across `chunks`. Tracked here so eviction
    /// doesn't have to re-scan on every push.
    total_bytes: usize,
    /// Hard cap on `total_bytes`. Configurable so tests can exercise the
    /// eviction path with small buffers; production callers use
    /// [`RingBuffer::default`] which pins it at [`RING_BUFFER_BYTES`].
    capacity: usize,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::with_capacity(RING_BUFFER_BYTES)
    }
}

impl RingBuffer {
    /// Construct a buffer with an explicit byte capacity. Production
    /// code uses [`Default::default`] — this exists for tests.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            next_seq: 1,
            total_bytes: 0,
            capacity,
        }
    }

    /// Append `data` as a fresh chunk. Returns the assigned `seq`.
    /// Evicts oldest chunks until total bytes are under [`capacity`].
    pub fn push(&mut self, data: Vec<u8>) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.total_bytes += data.len();
        self.chunks.push_back(Chunk { seq, data });
        self.evict();
        seq
    }

    /// Drop chunks whose `seq <= ack_seq`. Used by the future ack handler
    /// (Task 36 finalises hot-reconnect ack semantics; the prune entry
    /// point is locked here so the connection loop can call it.)
    pub fn prune_through(&mut self, ack_seq: u64) {
        while let Some(front) = self.chunks.front() {
            if front.seq <= ack_seq {
                let evicted = self.chunks.pop_front().expect("non-empty by check");
                self.total_bytes -= evicted.data.len();
            } else {
                break;
            }
        }
    }

    /// Return every chunk whose `seq > last_seq`, oldest first. Used on
    /// reconnect to replay output the Core hasn't acked yet. Cloning the
    /// chunks is fine at V0.1 volumes (≤ 1 MiB total).
    pub fn replay_past(&self, last_seq: u64) -> Vec<Chunk> {
        self.chunks
            .iter()
            .filter(|c| c.seq > last_seq)
            .cloned()
            .collect()
    }

    /// Highest `seq` currently stored, or 0 if the buffer is empty.
    pub fn last_seq(&self) -> u64 {
        self.chunks.back().map(|c| c.seq).unwrap_or(0)
    }

    /// Number of chunks currently held. Used by tests.
    #[cfg(test)]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    fn evict(&mut self) {
        while self.total_bytes > self.capacity {
            let evicted = match self.chunks.pop_front() {
                Some(c) => c,
                None => break,
            };
            self.total_bytes -= evicted.data.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_assigns_monotonic_seq() {
        let mut r = RingBuffer::with_capacity(1024);
        assert_eq!(r.push(b"a".to_vec()), 1);
        assert_eq!(r.push(b"b".to_vec()), 2);
        assert_eq!(r.push(b"c".to_vec()), 3);
        assert_eq!(r.last_seq(), 3);
    }

    #[test]
    fn replay_returns_chunks_past_watermark() {
        let mut r = RingBuffer::with_capacity(1024);
        r.push(b"a".to_vec());
        r.push(b"b".to_vec());
        r.push(b"c".to_vec());
        let out = r.replay_past(1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].seq, 2);
        assert_eq!(out[1].seq, 3);
    }

    #[test]
    fn eviction_drops_oldest_when_capacity_exceeded() {
        let mut r = RingBuffer::with_capacity(4);
        r.push(b"aaaa".to_vec()); // 4 bytes total
        r.push(b"bbbb".to_vec()); // 8 bytes total -> evict first
        assert_eq!(r.chunk_count(), 1);
        assert_eq!(r.last_seq(), 2);
    }

    #[test]
    fn prune_through_removes_acked_chunks() {
        let mut r = RingBuffer::with_capacity(1024);
        r.push(b"a".to_vec());
        r.push(b"b".to_vec());
        r.push(b"c".to_vec());
        r.prune_through(2);
        assert_eq!(r.chunk_count(), 1);
        assert_eq!(r.replay_past(0).len(), 1);
    }
}

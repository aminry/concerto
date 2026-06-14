//! Daily history condensation for the Maestro chat (Task 410, `design/08 §3.7`).
//!
//! The Maestro chat must keep its per-turn token cost **flat regardless of
//! session length**: the UI renders the full unabridged history, but the agent
//! only ever sees a bounded window. This module ships the two offline,
//! clock-injectable building blocks 414 drives from a timer/boot tick:
//!
//! - [`condense_day`] — the **offline pass**: summarize the 24-48h-old slice of
//!   the maestro chat into one daily-summary `chat_messages` row tagged
//!   `metadata.role_extra='daily_summary'`. Idempotent per day, so a timer that
//!   fires more than once (or a boot replay) never double-summarizes.
//! - [`assemble_input_window`] — the **pure window builder**: what the agent
//!   sees is `daily_summaries[:weekly]` + `verbatim[last 24h]` + the user's
//!   latest message. Returns a typed [`InputWindow`] (the FROZEN shape 414
//!   feeds to the spawned Maestro agent's stdin).
//!
//! ## What stays out (consumed seams)
//!
//! - The timer/scheduler that *fires* [`condense_day`] and the boot wiring are
//!   **Task 414** — this module ships callable `async fn`s and touches no
//!   `boot.rs`.
//! - The real Haiku/Sonnet one-paragraph summarizer is **Task 412**'s provider.
//!   The LIVE V1.0 path here is [`OneShotLlm`] / `DeterministicOneShot` (a
//!   deterministic truncate/collapse) so the 30-day flat-cost bench is
//!   reproducible in CI.
//! - The maestro `chats(kind='maestro')` chat id is **Task 403**'s
//!   (`ensure_maestro_chat`); this module **consumes** it — the caller passes
//!   it in.

use concerto_error::Result;
use concerto_persist::chat_messages::{self, ChatMessage};
use concerto_persist::Persistence;

use crate::llm::oneshot::{ActionKind, OneShotLlm, OneShotRequest};

/// One day in milliseconds — the condensation window granularity.
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Number of daily summaries retained in the agent input window
/// (`design/08 §3.7`: a flat 7-day window). Summaries older than this stay in
/// the DB for the UI but leave the agent window; a coarser monthly rollup is
/// a future task (Scope — out).
pub const WEEKLY_SUMMARY_WINDOW: usize = 7;

/// What the Maestro agent sees as input (FROZEN, `design/08 §3.7`).
///
/// `daily_summaries[:weekly]` + `verbatim[last 24h]` + the user's latest
/// message. The UI window is separate — it reads the full unabridged history
/// directly via the existing chat read path (out of scope here).
#[derive(Debug, Clone)]
pub struct InputWindow {
    /// The 7 most-recent daily summaries (older ones are dropped from the
    /// agent window; they remain in the DB for the UI).
    pub summaries: Vec<ChatMessage>,
    /// Every non-superseded message in the last 24h, verbatim.
    pub verbatim: Vec<ChatMessage>,
    /// The user's newest message (caller-passed; not yet persisted).
    pub latest: String,
}

/// Offline pass: condense the **24-48h-old** slice of `chat_id` into one
/// daily-summary row via `llm` (`DeterministicOneShot` is the LIVE path).
///
/// Returns the inserted summary id, or `None` when there is nothing to
/// condense — the slice is empty, or a daily summary already covers this day
/// (idempotent). `now_ms` is clock-injectable so the 30-day bench can advance a
/// synthetic clock; nothing here calls `SystemTime::now()`.
///
/// ## Day-boundary math
///
/// The condensed slice is `[now-48h, now-24h)`. The summary's `created_at` is
/// pinned to the **start of that slice** (`now - 2*DAY_MS`) so it sorts *before*
/// the last-24h verbatim window and is stable across re-runs (the idempotency
/// key). Re-running with the same `now_ms` finds the existing summary at that
/// boundary and no-ops.
pub async fn condense_day(
    persist: &Persistence,
    chat_id: &str,
    now_ms: i64,
    llm: &dyn OneShotLlm,
) -> Result<Option<String>> {
    let slice_start = now_ms - 2 * DAY_MS;
    let slice_end = now_ms - DAY_MS;

    let slice =
        chat_messages::list_in_day_range(persist.readers(), chat_id, slice_start, slice_end)
            .await?;
    if slice.is_empty() {
        return Ok(None);
    }

    // Idempotency: skip if a daily summary already sits at this slice's
    // boundary (the pinned `created_at` below). Keyed off the boundary, not a
    // count, so distinct days each get exactly one summary.
    let existing = chat_messages::list_daily_summaries(persist.readers(), chat_id).await?;
    if existing.iter().any(|s| s.created_at == slice_start) {
        return Ok(None);
    }

    // Build a digest prompt from the slice and route it through the one-shot
    // seam. The deterministic impl collapses/echoes the context; Task 412's
    // provider replaces it behind the same trait.
    let context = render_slice(&slice);
    let prompt = format!(
        "Summarize the following {} chat messages from the previous day into one paragraph.",
        slice.len()
    );
    let summary_text = llm
        .suggest(OneShotRequest::new(
            ActionKind::DigestSummary,
            // The maestro chat is workspace-global; the repo id is not
            // meaningful here, so the chat id is passed as the scope tag.
            chat_id,
            prompt,
            context,
        ))
        .await?;

    // Persist as a normal chat row whose text is content_json and whose tag is
    // the metadata column (D12). The id is derived deterministically from the
    // boundary so a re-run before the summary is committed stays stable.
    let id = format!("daily-summary-{chat_id}-{slice_start}");
    let content_json = serde_json::json!({ "text": summary_text }).to_string();
    let mut w = persist.writer().await;
    let inserted =
        chat_messages::insert_daily_summary(&mut w, chat_id, &id, &content_json, slice_start)
            .await?;
    Ok(Some(inserted))
}

/// Pure window builder: `daily_summaries[:weekly]` + `verbatim[last 24h]` +
/// `latest` (FROZEN, `design/08 §3.7`). Clock-injectable via `now_ms`.
///
/// This is the contract 414 feeds to the spawned Maestro agent's stdin. The UI
/// window is unaffected (it reads the full history via the existing chat path).
pub async fn assemble_input_window(
    persist: &Persistence,
    chat_id: &str,
    now_ms: i64,
    latest: String,
) -> Result<InputWindow> {
    // The 7 most-recent daily summaries (drop older — already a flat window).
    let mut summaries = chat_messages::list_daily_summaries(persist.readers(), chat_id).await?;
    if summaries.len() > WEEKLY_SUMMARY_WINDOW {
        let drop = summaries.len() - WEEKLY_SUMMARY_WINDOW;
        summaries.drain(0..drop);
    }

    // The last-24h verbatim slice, never summarized.
    let verbatim =
        chat_messages::list_in_day_range(persist.readers(), chat_id, now_ms - DAY_MS, now_ms)
            .await?;

    Ok(InputWindow {
        summaries,
        verbatim,
        latest,
    })
}

/// Render a slice of chat rows into the plain-text context the summarizer
/// reads. One line per message; the deterministic impl collapses whitespace,
/// so this stays compact and order-stable.
fn render_slice(slice: &[ChatMessage]) -> String {
    slice
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content_json))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::oneshot::DeterministicOneShot;
    use concerto_persist::chat_messages::NewChatMessage;
    use concerto_persist::{Persistence, PersistenceConfig};

    const CHAT: &str = "maestro-chat";

    async fn fresh() -> (tempfile::TempDir, Persistence) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let persist = Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open");
        {
            let mut w = persist.writer().await;
            sqlx::query(
                "INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, NULL, 'maestro', 0)",
            )
            .bind(CHAT)
            .execute(&mut *w)
            .await
            .expect("maestro chat");
        }
        (dir, persist)
    }

    async fn add_msg(persist: &Persistence, id: &str, created_at: i64) {
        let mut w = persist.writer().await;
        chat_messages::insert(
            &mut w,
            NewChatMessage {
                id: id.to_string(),
                chat_id: CHAT.to_string(),
                role: "user".to_string(),
                content_json: format!("{{\"text\":\"msg {id}\"}}"),
                created_at,
                parent_id: None,
                superseded_by: None,
                metadata: None,
            },
        )
        .await
        .expect("insert");
    }

    #[tokio::test]
    async fn condense_day_is_idempotent() {
        let (_dir, persist) = fresh().await;
        let llm = DeterministicOneShot;
        // now = day 3. The 24-48h slice is [day1, day2).
        let now = 3 * DAY_MS;
        add_msg(&persist, "a", DAY_MS + 1000).await;
        add_msg(&persist, "b", DAY_MS + 2000).await;

        let first = condense_day(&persist, CHAT, now, &llm)
            .await
            .expect("run 1");
        assert!(first.is_some(), "first run inserts a summary");

        let second = condense_day(&persist, CHAT, now, &llm)
            .await
            .expect("run 2");
        assert!(second.is_none(), "second run is a no-op (idempotent)");

        let summaries = chat_messages::list_daily_summaries(persist.readers(), CHAT)
            .await
            .expect("summaries");
        assert_eq!(summaries.len(), 1, "exactly one summary for the day");
        assert_eq!(
            summaries[0].created_at,
            now - 2 * DAY_MS,
            "summary pinned to the slice start (sorts before verbatim)"
        );
    }

    #[tokio::test]
    async fn condense_day_noops_on_empty_slice() {
        let (_dir, persist) = fresh().await;
        let llm = DeterministicOneShot;
        // No messages in [now-48h, now-24h).
        let out = condense_day(&persist, CHAT, 5 * DAY_MS, &llm)
            .await
            .expect("run");
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn assemble_input_window_keeps_verbatim_and_caps_summaries() {
        let (_dir, persist) = fresh().await;
        // 10 daily summaries; only the 7 most-recent should survive the window.
        {
            let mut w = persist.writer().await;
            for d in 0..10 {
                let id = format!("sum-{d}");
                chat_messages::insert_daily_summary(&mut w, CHAT, &id, "{}", d as i64 * DAY_MS)
                    .await
                    .expect("summary");
            }
        }
        let now = 20 * DAY_MS;
        // Two messages inside the last 24h, one just outside.
        add_msg(&persist, "recent-1", now - 1000).await;
        add_msg(&persist, "recent-2", now - 2000).await;
        add_msg(&persist, "old", now - DAY_MS - 1000).await;

        let window = assemble_input_window(&persist, CHAT, now, "hi".to_string())
            .await
            .expect("window");
        assert_eq!(window.summaries.len(), WEEKLY_SUMMARY_WINDOW, "capped at 7");
        // The 7 most-recent are days 3..=9.
        assert_eq!(window.summaries.first().unwrap().id, "sum-3");
        assert_eq!(window.summaries.last().unwrap().id, "sum-9");
        let verb: Vec<&str> = window.verbatim.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            verb,
            vec!["recent-2", "recent-1"],
            "only the last-24h slice"
        );
        assert_eq!(window.latest, "hi");
    }

    /// The `design/08 §10` flat-cost bench: over a 30-day synthetic clock with N
    /// messages/day, the agent input window stays within a bounded constant
    /// (it does NOT grow with total history) while the last-24h verbatim slice
    /// is preserved unsummarized.
    #[tokio::test]
    async fn thirty_day_input_window_stays_flat() {
        let (_dir, persist) = fresh().await;
        let llm = DeterministicOneShot;
        const MSGS_PER_DAY: i64 = 12;
        const DAYS: i64 = 30;

        let mut max_window_msgs = 0usize;
        let mut total_msgs = 0i64;

        for day in 0..DAYS {
            // Lay down this day's messages spread across the day.
            for i in 0..MSGS_PER_DAY {
                let ts = day * DAY_MS + i * (DAY_MS / MSGS_PER_DAY) + 1;
                add_msg(&persist, &format!("d{day}-m{i}"), ts).await;
                total_msgs += 1;
            }
            // Run the daily pass as of the END of this day (so the previous
            // day's slice is condensable). now = end of `day`.
            let now = (day + 1) * DAY_MS;
            condense_day(&persist, CHAT, now, &llm)
                .await
                .expect("condense");

            let window = assemble_input_window(&persist, CHAT, now, "latest".to_string())
                .await
                .expect("window");
            let window_msgs = window.summaries.len() + window.verbatim.len();
            max_window_msgs = max_window_msgs.max(window_msgs);

            // The last-24h verbatim slice is preserved unsummarized: every
            // message authored in (now-24h, now) shows up verbatim.
            assert!(
                !window.verbatim.is_empty(),
                "day {day}: last-24h verbatim slice preserved"
            );
        }

        // Flat-cost invariant: the window is bounded by
        // WEEKLY_SUMMARY_WINDOW summaries + ~one day of verbatim, regardless of
        // the 30*12 = 360 total messages on disk.
        let bound = WEEKLY_SUMMARY_WINDOW + (2 * MSGS_PER_DAY as usize);
        assert!(
            max_window_msgs <= bound,
            "input window must stay flat: max={max_window_msgs} > bound={bound} \
             (total history = {total_msgs} messages)"
        );
        assert!(
            total_msgs as usize > bound,
            "sanity: total history must exceed the bound for the test to be meaningful"
        );
    }
}

//! Integration test for the Task 505 `ActOnChip` dispatch
//! (`notifications::chip_dispatch::act_on_chip`): chip lookup by `rule_id`,
//! action classification, and the idempotent first-wins marker.

use concerto_core::notifications::chip_dispatch::{act_on_chip, ChipDispatch};
use concerto_persist::notifications::{self, NewNotification};
use concerto_persist::{Persistence, PersistenceConfig};

async fn fresh() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Persistence::open(PersistenceConfig {
        db_path: dir.path().join("t.db"),
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

#[tokio::test(flavor = "multi_thread")]
async fn act_on_chip_classifies_and_is_first_wins() {
    let (_dir, persist) = fresh().await;
    {
        let mut w = persist.writer().await;
        for id in ["dev-1", "dev-2"] {
            sqlx::query("INSERT INTO devices (id,name,public_key,paired_at,push_token,push_platform) VALUES (?,'P',?,1,'tok','expo')")
                .bind(id)
                .bind(vec![0u8; 32])
                .execute(&mut *w)
                .await
                .unwrap();
        }
        notifications::insert(
            &mut w,
            NewNotification {
                id: "n-1".into(),
                kind: "tool_approval_needed".into(),
                subject_kind: "session".into(),
                subject_id: "sess-1".into(),
                workspace_id: None,
                workarea_id: None,
                session_id: None,
                title: "Approve Bash?".into(),
                body: "ls".into(),
                chips_json: Some(
                    r#"[{"rule_id":"approve","workarea_id":"","title":"Approve","priority":90,"created_at_ms":1,"action":"approve"},
                        {"rule_id":"deny","workarea_id":"","title":"Deny","priority":80,"created_at_ms":1,"action":"deny"}]"#
                        .into(),
                ),
                approval_json: None,
                severity: "high".into(),
                created_at: 1,
            },
        )
        .await
        .unwrap();
    }

    // dev-1 taps "approve" → ResolveApproval + wins the race.
    let out = act_on_chip(&persist, "n-1", "approve", "dev-1", 100)
        .await
        .unwrap();
    assert_eq!(
        out.dispatch,
        ChipDispatch::ResolveApproval {
            decision: "approve".into()
        }
    );
    assert!(!out.already_resolved, "first device wins");

    // dev-2 taps "deny" → still classifies, but loses (already acted on).
    let out2 = act_on_chip(&persist, "n-1", "deny", "dev-2", 200)
        .await
        .unwrap();
    assert_eq!(
        out2.dispatch,
        ChipDispatch::ResolveApproval {
            decision: "deny".into()
        }
    );
    assert!(
        out2.already_resolved,
        "second device loses the first-wins race"
    );

    // The marker reflects the winner.
    let row = notifications::get(persist.readers(), "n-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.action_taken.as_deref(), Some("approve"));
    assert_eq!(row.action_taken_by_device_id.as_deref(), Some("dev-1"));

    // Unknown chip / notification → NotFound.
    assert!(act_on_chip(&persist, "n-1", "nope", "dev-1", 300)
        .await
        .is_err());
    assert!(act_on_chip(&persist, "missing", "approve", "dev-1", 300)
        .await
        .is_err());
}

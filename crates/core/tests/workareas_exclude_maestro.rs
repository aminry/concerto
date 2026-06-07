//! Task 311: the per-workarea `exclude_from_maestro` privacy toggle.
//!
//! Tier-1, in-process tests against a real `WorkareaManager` + the real
//! `WorkareasHandler` over a tempdir SQLite DB. No git, no agent host, no
//! gRPC server — workarea rows are seeded directly so the tests focus on the
//! load-bearing details:
//!
//! - `set_exclude_from_maestro(true)` then read-back via `get` / the proto;
//! - the read-modify-write **preserves sibling keys** (`files_to_copy_applied`
//!   survives a toggle) — the one detail that breaks if you call the
//!   whole-blob `set_settings_json`;
//! - `set_exclude_from_maestro(false)` clears it;
//! - empty / `{}` / malformed `settings_json` toggles cleanly;
//! - the derived proto bool is populated on `GetWorkarea` / `ListWorkareas`.
//!
//! Storage stays in `workareas.settings_json` (design/03 §3.14, no migration);
//! Maestro enforcement is Task 413 (Phase 4) — out of scope here.

#![cfg(unix)]

use std::sync::Arc;

use concerto_core::handlers::workareas::WorkareasHandler;
use concerto_core::repo_manager::RepoManager;
use concerto_core::workspace_manager::WorkareaManager;
use concerto_persist::{workareas, Persistence, PersistenceConfig, WorkareaId};
use concerto_proto::v1::workareas_server::Workareas as WorkareasService;
use concerto_proto::v1::{
    ListWorkareasRequest, SetWorkareaExcludeFromMaestroRequest, WorkareaId as ProtoWorkareaId,
};
use serde_json::Value;
use tempfile::TempDir;
use tonic::Request;

struct Fixture {
    _tmp: TempDir,
    persistence: Arc<Persistence>,
    mgr: WorkareaManager,
    handler: WorkareasHandler,
}

/// Build a manager + handler over a tempdir DB with a project + workspace
/// seeded (so foreign keys hold) but no repos — workarea rows are inserted
/// directly with the `settings_json` each test needs.
async fn make_fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let db_path = data_dir.join("concerto.db");
    let persistence = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open"),
    );

    {
        let mut w = persistence.writer().await;
        sqlx::query("INSERT INTO projects (id, name, created_at) VALUES ('p', 'p', 0)")
            .execute(&mut *w)
            .await
            .expect("project");
        sqlx::query(
            "INSERT INTO workspaces (id, project_id, name, slug, created_at)
             VALUES ('ws', 'p', 'ws', 'ws', 0)",
        )
        .execute(&mut *w)
        .await
        .expect("workspace");
    }

    let repo_manager = RepoManager::new(Arc::clone(&persistence), data_dir.join("repos"));
    let mgr = WorkareaManager::new(
        Arc::clone(&persistence),
        repo_manager,
        Arc::new(data_dir),
        Arc::new(tmp.path().join("config")),
    );
    let handler = WorkareasHandler::new(mgr.clone());

    Fixture {
        _tmp: tmp,
        persistence,
        mgr,
        handler,
    }
}

/// Insert a workarea row with the given `settings_json` blob and return its id.
async fn seed_workarea(fx: &Fixture, composer: &str, settings_json: &str) -> WorkareaId {
    let id = WorkareaId(format!("wa-{composer}"));
    let mut w = fx.persistence.writer().await;
    sqlx::query(
        "INSERT INTO workareas
            (id, workspace_id, composer_name, branch_name, worktree_root, status,
             created_at, settings_json)
         VALUES (?, 'ws', ?, ?, '/tmp/wt', 'active', 0, ?)",
    )
    .bind(&id.0)
    .bind(composer)
    .bind(format!("concerto/{composer}"))
    .bind(settings_json)
    .execute(&mut *w)
    .await
    .expect("insert workarea");
    id
}

/// The raw `settings_json` blob for a workarea, parsed to a JSON object.
async fn settings_obj(fx: &Fixture, id: &WorkareaId) -> serde_json::Map<String, Value> {
    let row = workareas::get(fx.persistence.readers(), id)
        .await
        .expect("get")
        .expect("row");
    match serde_json::from_str::<Value>(&row.settings_json).expect("parse settings_json") {
        Value::Object(map) => map,
        other => panic!("settings_json must be an object, got {other:?}"),
    }
}

#[tokio::test]
async fn set_true_then_read_back_via_get_and_proto() {
    let fx = make_fixture().await;
    let id = seed_workarea(&fx, "bach", "{}").await;

    // Manager returns the updated row with the flag set in settings_json.
    let updated = fx.mgr.set_exclude_from_maestro(&id, true).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&updated.settings_json).unwrap()["exclude_from_maestro"],
        Value::Bool(true)
    );

    // The proto projection (via the real GetWorkarea handler) carries it.
    let resp = fx
        .handler
        .get_workarea(Request::new(ProtoWorkareaId {
            value: id.0.clone(),
        }))
        .await
        .expect("get_workarea")
        .into_inner();
    assert_eq!(resp.exclude_from_maestro, Some(true));
}

#[tokio::test]
async fn read_modify_write_preserves_sibling_keys() {
    let fx = make_fixture().await;
    // A workarea whose settings_json already carries files_to_copy_applied.
    let id = seed_workarea(&fx, "ravel", r#"{"files_to_copy_applied":true}"#).await;

    fx.mgr.set_exclude_from_maestro(&id, true).await.unwrap();

    let obj = settings_obj(&fx, &id).await;
    assert_eq!(
        obj.get("files_to_copy_applied"),
        Some(&Value::Bool(true)),
        "the sibling key must survive the read-modify-write"
    );
    assert_eq!(obj.get("exclude_from_maestro"), Some(&Value::Bool(true)));

    // Toggling again (to false) still leaves the sibling intact.
    fx.mgr.set_exclude_from_maestro(&id, false).await.unwrap();
    let obj = settings_obj(&fx, &id).await;
    assert_eq!(obj.get("files_to_copy_applied"), Some(&Value::Bool(true)));
    assert_eq!(obj.get("exclude_from_maestro"), Some(&Value::Bool(false)));
}

#[tokio::test]
async fn set_false_clears_the_flag_in_the_proto() {
    let fx = make_fixture().await;
    let id = seed_workarea(&fx, "satie", r#"{"exclude_from_maestro":true}"#).await;

    let updated = fx.mgr.set_exclude_from_maestro(&id, false).await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&updated.settings_json).unwrap()["exclude_from_maestro"],
        Value::Bool(false)
    );

    let resp = fx
        .handler
        .get_workarea(Request::new(ProtoWorkareaId {
            value: id.0.clone(),
        }))
        .await
        .expect("get_workarea")
        .into_inner();
    assert_eq!(resp.exclude_from_maestro, Some(false));
}

#[tokio::test]
async fn empty_object_and_malformed_blobs_toggle_cleanly() {
    let fx = make_fixture().await;

    // `{}` (the migration-0002 default), a malformed blob, and a non-object
    // blob all start from `{}` and end with a clean single-key object.
    for (composer, blob) in [
        ("empty", "{}"),
        ("malformed", "not json at all"),
        ("array", "[1,2,3]"),
        ("scalar", "true"),
    ] {
        let id = seed_workarea(&fx, composer, blob).await;
        fx.mgr.set_exclude_from_maestro(&id, true).await.unwrap();

        let obj = settings_obj(&fx, &id).await;
        assert_eq!(
            obj.get("exclude_from_maestro"),
            Some(&Value::Bool(true)),
            "{composer}: toggle must write a clean bool"
        );
        assert_eq!(
            obj.len(),
            1,
            "{composer}: a malformed/non-object blob is discarded → just the one key"
        );
    }
}

#[tokio::test]
async fn proto_field_populated_on_get_and_list() {
    let fx = make_fixture().await;
    let excluded = seed_workarea(&fx, "alpha", r#"{"exclude_from_maestro":true}"#).await;
    let _visible = seed_workarea(&fx, "beta", "{}").await;

    // GetWorkarea on the excluded one.
    let got = fx
        .handler
        .get_workarea(Request::new(ProtoWorkareaId {
            value: excluded.0.clone(),
        }))
        .await
        .expect("get")
        .into_inner();
    assert_eq!(got.exclude_from_maestro, Some(true));

    // ListWorkareas populates the field on every member (Some, never None).
    let listed = fx
        .handler
        .list_workareas(Request::new(ListWorkareasRequest {
            workspace_id: "ws".to_string(),
            include_archived: false,
        }))
        .await
        .expect("list")
        .into_inner()
        .workareas;
    assert_eq!(listed.len(), 2);
    for wa in &listed {
        assert!(
            wa.exclude_from_maestro.is_some(),
            "every listed workarea must carry the derived bool"
        );
    }
    let alpha = listed.iter().find(|w| w.composer_name == "alpha").unwrap();
    let beta = listed.iter().find(|w| w.composer_name == "beta").unwrap();
    assert_eq!(alpha.exclude_from_maestro, Some(true));
    assert_eq!(beta.exclude_from_maestro, Some(false));
}

#[tokio::test]
async fn toggle_rpc_round_trips_through_the_handler() {
    let fx = make_fixture().await;
    let id = seed_workarea(&fx, "holst", "{}").await;

    let resp = fx
        .handler
        .set_workarea_exclude_from_maestro(Request::new(SetWorkareaExcludeFromMaestroRequest {
            workarea_id: id.0.clone(),
            exclude: true,
        }))
        .await
        .expect("rpc")
        .into_inner();
    assert_eq!(resp.exclude_from_maestro, Some(true));

    // Empty id is rejected.
    let err = fx
        .handler
        .set_workarea_exclude_from_maestro(Request::new(SetWorkareaExcludeFromMaestroRequest {
            workarea_id: String::new(),
            exclude: true,
        }))
        .await
        .expect_err("empty id must be rejected");
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

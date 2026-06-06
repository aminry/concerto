//! Round-trip test for the Task 313 `vcs_credentials` accessor (migration
//! 0012): `upsert` → `get` / `get_by_scope` / `list_by_provider`, the
//! `UNIQUE(provider, scope_id)` upsert semantics (a second upsert on the same
//! natural key UPDATEs in place, preserving the primary key + `created_at`), and
//! the metadata-only invariant (no secret columns exist to write).

use concerto_persist::{vcs_credentials, NewVcsCredential, Persistence, PersistenceConfig};

async fn fresh_db() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let persist = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

#[tokio::test]
async fn upsert_get_round_trip() {
    let (_dir, persist) = fresh_db().await;

    let id = {
        let mut w = persist.writer().await;
        vcs_credentials::upsert(
            &mut w,
            NewVcsCredential {
                id: concerto_persist::VcsCredentialId("cred-1".to_string()),
                provider: "github".to_string(),
                scope_id: "app-42".to_string(),
                external_account: Some("acme-org".to_string()),
                app_id: Some("42".to_string()),
                installation_id: Some("9001".to_string()),
                token_expires_at: Some(1_700_000_000_000),
                created_at: 1_699_000_000_000,
                updated_at: 1_699_000_000_000,
            },
        )
        .await
        .expect("upsert")
    };

    let got = vcs_credentials::get(persist.readers(), &id)
        .await
        .expect("get")
        .expect("row present");
    assert_eq!(got.provider, "github");
    assert_eq!(got.scope_id, "app-42");
    assert_eq!(got.external_account.as_deref(), Some("acme-org"));
    assert_eq!(got.app_id.as_deref(), Some("42"));
    assert_eq!(got.installation_id.as_deref(), Some("9001"));
    assert_eq!(got.token_expires_at, Some(1_700_000_000_000));

    // get_by_scope resolves the same row by its natural key.
    let by_scope = vcs_credentials::get_by_scope(persist.readers(), "github", "app-42")
        .await
        .expect("get_by_scope")
        .expect("row present");
    assert_eq!(by_scope.id, id);
}

#[tokio::test]
async fn upsert_is_idempotent_on_natural_key() {
    let (_dir, persist) = fresh_db().await;

    let first_id = {
        let mut w = persist.writer().await;
        vcs_credentials::upsert(
            &mut w,
            NewVcsCredential {
                id: concerto_persist::VcsCredentialId("cred-a".to_string()),
                provider: "linear".to_string(),
                scope_id: "acct-1".to_string(),
                external_account: None,
                app_id: None,
                installation_id: None,
                token_expires_at: None,
                created_at: 1_699_000_000_000,
                updated_at: 1_699_000_000_000,
            },
        )
        .await
        .expect("first upsert")
    };

    // Second upsert with a DIFFERENT id but the same (provider, scope_id):
    // UNIQUE(provider, scope_id) forces an UPDATE, keeping the original id.
    let second_id = {
        let mut w = persist.writer().await;
        vcs_credentials::upsert(
            &mut w,
            NewVcsCredential {
                id: concerto_persist::VcsCredentialId("cred-b".to_string()),
                provider: "linear".to_string(),
                scope_id: "acct-1".to_string(),
                external_account: Some("updated".to_string()),
                app_id: None,
                installation_id: None,
                token_expires_at: Some(123),
                created_at: 1_700_000_000_000, // ignored on UPDATE
                updated_at: 1_700_000_000_000,
            },
        )
        .await
        .expect("second upsert")
    };
    assert_eq!(
        first_id, second_id,
        "upsert on the same natural key must keep the original primary key"
    );

    let got = vcs_credentials::get(persist.readers(), &first_id)
        .await
        .expect("get")
        .expect("row present");
    assert_eq!(got.external_account.as_deref(), Some("updated"));
    assert_eq!(got.token_expires_at, Some(123));
    assert_eq!(
        got.created_at, 1_699_000_000_000,
        "created_at preserved across the UPDATE"
    );
    assert_eq!(got.updated_at, 1_700_000_000_000, "updated_at advanced");
}

#[tokio::test]
async fn list_by_provider_filters_and_orders() {
    let (_dir, persist) = fresh_db().await;

    {
        let mut w = persist.writer().await;
        for (id, provider, scope) in [
            ("c1", "jira", "z-acct"),
            ("c2", "jira", "a-acct"),
            ("c3", "github", "app-1"),
        ] {
            vcs_credentials::upsert(
                &mut w,
                NewVcsCredential {
                    id: concerto_persist::VcsCredentialId(id.to_string()),
                    provider: provider.to_string(),
                    scope_id: scope.to_string(),
                    external_account: None,
                    app_id: None,
                    installation_id: None,
                    token_expires_at: None,
                    created_at: 1,
                    updated_at: 1,
                },
            )
            .await
            .expect("upsert");
        }
    }

    let jira = vcs_credentials::list_by_provider(persist.readers(), "jira")
        .await
        .expect("list");
    assert_eq!(jira.len(), 2, "only jira rows");
    assert_eq!(
        jira.iter().map(|c| c.scope_id.as_str()).collect::<Vec<_>>(),
        vec!["a-acct", "z-acct"],
        "ordered by scope_id"
    );
}

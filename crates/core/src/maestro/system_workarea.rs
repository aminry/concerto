//! The reserved, UI-hidden system workspace + workarea that hosts the global
//! Maestro session. Satisfies `sessions.workarea_id NOT NULL REFERENCES
//! workareas(id)` without a schema change (design Fork B1). The sentinel ids are
//! filtered from every user-facing list (a separate task wires that filtering).

use concerto_error::Result;
use concerto_persist::{
    NewWorkarea, NewWorkspace, Persistence, WorkareaId, WorkspaceId, workareas, workspaces,
};

// Re-export the canonical sentinel id literals from `concerto_persist` so that
// the single source of truth lives in the persistence layer (which is the only
// layer that needs to reference them in SQL). All existing call sites resolve
// through these names unchanged.
pub use concerto_persist::MAESTRO_SYSTEM_WORKAREA_ID as SYSTEM_WORKAREA_ID;
pub use concerto_persist::MAESTRO_SYSTEM_WORKSPACE_ID as SYSTEM_WORKSPACE_ID;

/// Reserved slug + composer/branch names for the system rows. The slug must be
/// globally unique (`workspaces.UNIQUE(slug)`) and the composer name unique
/// within the workspace (`workareas.UNIQUE(workspace_id, composer_name)`) — the
/// sentinels are reserved so no user-created row can collide.
const SYSTEM_WORKSPACE_SLUG: &str = "__maestro_system__";
const SYSTEM_COMPOSER_NAME: &str = "__maestro__";
const SYSTEM_BRANCH_NAME: &str = "__maestro__";

/// Idempotently ensure the reserved system workspace + workarea exist, returning
/// their ids. Safe to call on every boot.
///
/// The Maestro is global — it has no real host workarea — but the agent
/// supervisor records its session against a `workareas` row whose FK is
/// `NOT NULL`. Rather than relax the schema, we reserve a hidden workspace +
/// workarea with stable sentinel ids (design Fork B1). Both rows are created
/// with inert defaults (no repos, not archived, status `created`); the workarea's
/// `worktree_root` points at the Maestro scratch dir since it is NOT a repo
/// worktree. Re-running this on a subsequent boot is a no-op (each row is only
/// inserted when absent).
pub async fn ensure_system_workspace_and_workarea(
    persist: &Persistence,
) -> Result<(WorkspaceId, WorkareaId)> {
    let ws_id = WorkspaceId(SYSTEM_WORKSPACE_ID.into());
    let wa_id = WorkareaId(SYSTEM_WORKAREA_ID.into());

    // Insert the reserved workspace if absent.
    if workspaces::get(persist.readers(), &ws_id).await?.is_none() {
        let mut w = persist.writer().await;
        workspaces::insert(
            &mut w,
            NewWorkspace {
                id: ws_id.clone(),
                name: "Maestro (system)".into(),
                slug: SYSTEM_WORKSPACE_SLUG.into(),
                icon: None,
                description: None,
                // Inherit defaults — the Maestro's own permission stance is the
                // session's strict mode (set at spawn), not a workspace default.
                permission_mode: None,
                created_at: 0,
            },
        )
        .await?;
    }

    // Insert the reserved workarea if absent.
    if workareas::get(persist.readers(), &wa_id).await?.is_none() {
        let worktree_root = crate::maestro::maestro_scratch_dir()?
            .to_string_lossy()
            .into_owned();
        let mut w = persist.writer().await;
        workareas::insert(
            &mut w,
            NewWorkarea {
                id: wa_id.clone(),
                workspace_id: ws_id.0.clone(),
                composer_name: SYSTEM_COMPOSER_NAME.into(),
                branch_name: SYSTEM_BRANCH_NAME.into(),
                // Not a repo worktree — points at the Maestro scratch dir.
                worktree_root,
                // Inert: `created` is the idle/system state (the Workspace
                // Manager's natural insert state); the Maestro session lives in
                // the supervisor, not in this row's lifecycle.
                status: "created".into(),
                permission_mode: None,
                created_at: 0,
            },
        )
        .await?;
    }

    Ok((ws_id, wa_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use concerto_persist::{Persistence, PersistenceConfig};

    async fn fresh() -> (tempfile::TempDir, Persistence) {
        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open");
        (dir, persist)
    }

    #[tokio::test]
    async fn ensure_is_idempotent_and_returns_sentinel_ids() {
        let (_dir, persist) = fresh().await;
        let (ws1, wa1) = ensure_system_workspace_and_workarea(&persist).await.unwrap();
        let (ws2, wa2) = ensure_system_workspace_and_workarea(&persist).await.unwrap();
        assert_eq!(ws1, ws2);
        assert_eq!(wa1, wa2);
        assert_eq!(ws1.0, SYSTEM_WORKSPACE_ID);
        assert_eq!(wa1.0, SYSTEM_WORKAREA_ID);
        assert!(
            concerto_persist::workareas::get(persist.readers(), &wa1)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            concerto_persist::workspaces::get(persist.readers(), &ws1)
                .await
                .unwrap()
                .is_some()
        );
    }
}

//! `/api/storage/*` — DB backup/restore for Settings → Storage. New to the Rust
//! app (the Python API never exposed these; only the CLI did). The frontend picks
//! paths with the OS save/open dialogs and sends them here as JSON.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use linxiv_core::service::db_admin;

use crate::route::{ApiError, ReqCtx};
use crate::state::AppState;

pub(crate) async fn handle(state: &AppState, ctx: &ReqCtx<'_>) -> Option<Result<Value, ApiError>> {
    match (ctx.method, ctx.segs) {
        ("POST", ["api", "storage", "backup"]) => Some(backup(state, ctx)),
        ("POST", ["api", "storage", "restore"]) => Some(restore(state, ctx)),
        _ => None,
    }
}

/// `POST /api/storage/backup` `{dest_path}` → `{path, bytes}`. Vacuums to a
/// temp file, then renames over dest. Temp path includes process ID for collision safety.
fn backup(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        dest_path: PathBuf,
    }
    let b: Body = ctx.parse_body()?;
    db_admin::reject_live_db(&b.dest_path, "dest_path", "destination")?;
    let temp_path = {
        let mut path = b.dest_path.clone();
        path.set_extension(format!("tmp-backup-{}", std::process::id()));
        path
    };
    let info = state.with_conn(|conn| {
        // Check/remove/rename sequence is serialized by the DB mutex.
        if temp_path.exists() {
            std::fs::remove_file(&temp_path)
                .map_err(|e| ApiError::new(400, format!("cannot remove stale temp file: {e}")))?;
        }
        let mut backup_info = db_admin::backup(conn, &temp_path)?;
        std::fs::rename(&temp_path, &b.dest_path).map_err(|e| {
            ApiError::new(
                400,
                format!(
                    "cannot finalize backup: {e} (backup file is at: {})",
                    temp_path.display()
                ),
            )
        })?;
        backup_info.path = b.dest_path.clone();
        Ok::<_, ApiError>(backup_info)
    })?;
    serde_json::to_value(&info)
        .map_err(|e| ApiError::new(500, format!("could not serialize backup result: {e}")))
}

/// `POST /api/storage/restore` `{src_path}` → `{ok}`. `db_admin::restore_in_place`
/// owns the park/swap/reopen sequence (shared with the MCP tool); the state mutex is
/// held throughout. The UI still tells the user to restart: in-flight frontend state
/// is not rewound by the swap.
fn restore(state: &AppState, ctx: &ReqCtx<'_>) -> Result<Value, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        src_path: PathBuf,
    }
    let b: Body = ctx.parse_body()?;
    db_admin::reject_live_db(&b.src_path, "src_path", "source")?;
    // Validate the backup source early, before parking the live connection.
    db_admin::validate_backup_source(&b.src_path)?;
    state.with_conn(|conn| db_admin::restore_in_place(conn, &b.src_path))?;
    Ok(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::{route, ApiRequest};
    use linxiv_core::config;
    use tempfile::TempDir;

    struct EnvVarGuard(&'static str);
    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    #[tokio::test]
    async fn backup_and_restore_scenarios() {
        let tmpdir = TempDir::new().unwrap();
        std::env::set_var("LINXIV_DATA_DIR", tmpdir.path());
        let _guard = EnvVarGuard("LINXIV_DATA_DIR");

        let db_path = config::db_path();
        let conn = db_admin::open_app_db().unwrap();
        let state = AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir());

        // Scenario 1: backup rejects live DB as destination
        let db_path_str = db_path.to_string_lossy();
        let result = route(
            &state,
            ApiRequest {
                method: "POST".into(),
                path: "/api/storage/backup".into(),
                body: Some(serde_json::json!({ "dest_path": db_path_str.as_ref() })),
            },
        )
        .await;
        assert!(
            result.is_err(),
            "should reject live DB as backup destination"
        );
        let err = result.unwrap_err();
        assert_eq!(err.status, 400);
        assert!(err.detail.contains("live database"));

        // Scenario 2: backup succeeds and overwrites an existing dest despite a stale leftover temp file
        let dest = tmpdir.path().join("backup.db");
        let original_content = b"important data";
        std::fs::write(&dest, original_content).unwrap();
        let temp = {
            let mut p = dest.clone();
            p.set_extension(format!("tmp-backup-{}", std::process::id()));
            p
        };
        std::fs::write(&temp, b"temp").unwrap();

        let dest_str = dest.to_string_lossy();
        let result = route(
            &state,
            ApiRequest {
                method: "POST".into(),
                path: "/api/storage/backup".into(),
                body: Some(serde_json::json!({ "dest_path": dest_str.as_ref() })),
            },
        )
        .await;
        assert!(
            result.is_ok(),
            "backup should succeed after cleaning stale temp"
        );
        let new_content = std::fs::read(&dest).unwrap();
        assert_ne!(&new_content, original_content);
        assert!(
            !temp.exists(),
            "temp file should not exist after successful backup"
        );

        // Scenario 3: restore happy path
        let backup_path = tmpdir.path().join("snapshot.db");
        state
            .with_conn(|conn| db_admin::backup(conn, &backup_path))
            .unwrap();
        assert!(backup_path.exists());

        let backup_str = backup_path.to_string_lossy();
        let result = route(
            &state,
            ApiRequest {
                method: "POST".into(),
                path: "/api/storage/restore".into(),
                body: Some(serde_json::json!({ "src_path": backup_str.as_ref() })),
            },
        )
        .await;
        assert!(result.is_ok(), "restore should succeed");
        let value = result.unwrap();
        assert!(value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));

        // Scenario 4: restore with invalid source leaves conn alive
        let invalid_path = tmpdir.path().join("nonexistent.db");
        let invalid_str = invalid_path.to_string_lossy();
        let result = route(
            &state,
            ApiRequest {
                method: "POST".into(),
                path: "/api/storage/restore".into(),
                body: Some(serde_json::json!({ "src_path": invalid_str.as_ref() })),
            },
        )
        .await;
        assert!(result.is_err(), "restore should fail with invalid source");

        let query_result = state.with_conn(|conn| {
            conn.query_row("SELECT count(*) FROM PAPER", [], |row| row.get::<_, i32>(0))
        });
        assert!(
            query_result.is_ok(),
            "connection should still be working after restore failure"
        );
    }
}

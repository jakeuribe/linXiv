//! Two-way share sync glue: per-share settings sidecars, the received→canonical
//! import entry point, the role-aware `sync_share` used by both the route arm
//! and the 5-minute interval task spawned from `main.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Manager;

use linxiv_core::service::project as project_svc;
use linxiv_share::{
    build_shared_project, doc_path, import_shared_project, received_dir, save, valid_share_id,
    ShareNode, ShareTicket,
};

use crate::route::share::ShareState;
use crate::route::ApiError;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    #[default]
    TwoWay,
    SharedToLocal,
    LocalToShared,
}

/// Per-share sync settings, stored as `share_dir/settings/<id>.json` — share-local
/// sidecar state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShareSettings {
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub direction: SyncDirection,
}

pub fn settings_path(share_dir: &Path, share_id: &str) -> PathBuf {
    share_dir.join("settings").join(format!("{share_id}.json"))
}

/// Ticket sidecar written at join time so re-sync knows the origin address.
pub fn ticket_path(share_dir: &Path, share_id: &str) -> PathBuf {
    received_dir(share_dir).join(format!("{share_id}.ticket"))
}

/// Missing sidecar → defaults (unpaused, two_way); unreadable/unparseable sidecar
/// → logged, paused.
pub fn load_settings(share_dir: &Path, share_id: &str) -> ShareSettings {
    let path = settings_path(share_dir, share_id);
    if !path.exists() {
        return ShareSettings::default();
    }
    let parsed = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()));
    parsed.unwrap_or_else(|e| {
        eprintln!("share settings {share_id}: unreadable sidecar, pausing sync: {e}");
        ShareSettings {
            paused: true,
            direction: SyncDirection::TwoWay,
        }
    })
}

pub fn save_settings(share_dir: &Path, share_id: &str, s: &ShareSettings) -> std::io::Result<()> {
    let path = settings_path(share_dir, share_id);
    std::fs::create_dir_all(path.parent().expect("settings path has a parent"))?;
    // Write to a sibling temp file then rename.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(s).expect("settings serialize"))?;
    std::fs::rename(&tmp, &path)
}

/// Import a received mirror into the canonical DB (find-or-create the linked
/// project by SHARE_ID). Manual import path. Returns the linked project_fk.
pub fn import_received(
    state: &AppState,
    share_dir: &Path,
    share_id: &str,
) -> Result<i64, ApiError> {
    if !valid_share_id(share_id) {
        return Err(ApiError::new(404, format!("share {share_id:?} not found")));
    }
    let sp = ShareNode::received(share_dir, share_id)?;
    Ok(state.with_conn(|conn| import_shared_project(conn, &sp))?)
}

/// Bump mtime — the UI reads the doc file's mtime as synced_at.
fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// One sync pass for one share, honoring role (hoster/reader, by which doc file
/// exists) and settings (paused + direction). Used by the route arm and the
/// interval task.
pub async fn sync_share(
    state: &AppState,
    share: &ShareState,
    share_id: &str,
) -> Result<Value, ApiError> {
    if !valid_share_id(share_id) {
        return Err(ApiError::new(404, format!("share {share_id:?} not found")));
    }
    let dir = share.share_dir().to_path_buf();
    let settings = load_settings(&dir, share_id);
    if settings.paused {
        return Ok(json!({ "synced": false, "reason": "paused" }));
    }
    let _guard = share.lock_writes(share_id).await;

    let hoster_doc = doc_path(&dir, share_id);
    let reader_doc = doc_path(&received_dir(&dir), share_id);
    if hoster_doc.is_file() && reader_doc.is_file() {
        return Err(ApiError::new(
            500,
            format!("share {share_id} has both a published and a received doc"),
        ));
    }

    if hoster_doc.is_file() {
        // Hoster leg = local_to_shared: rebuild + save + refresh the registry.
        // Its shared_to_local leg is a no-op until W4 editors give readers edits.
        if settings.direction == SyncDirection::SharedToLocal {
            return Ok(json!({ "synced": false, "reason": "direction", "role": "hoster" }));
        }
        let Some(fk) = state.with_conn(|c| project_svc::find_by_share_id(c, share_id))? else {
            // Doc + settings stay on disk; only explicit unpublish deletes them.
            return Ok(json!({ "synced": false, "reason": "project gone" }));
        };
        let sp = state.with_conn(|c| build_shared_project(c, fk))?;
        save(&dir, &sp)?;
        if let Some(node) = share.node().await {
            node.refresh(share_id).await?;
        }
        touch(&hoster_doc);
        return Ok(json!({ "synced": true, "role": "hoster" }));
    }

    if reader_doc.is_file() {
        // Reader leg = shared_to_local: refetch from the stored ticket, then
        // import into the linked project.
        if settings.direction == SyncDirection::LocalToShared {
            return Ok(json!({ "synced": false, "reason": "direction", "role": "reader" }));
        }
        let Ok(raw) = std::fs::read_to_string(ticket_path(&dir, share_id)) else {
            eprintln!("share sync {share_id}: no ticket file");
            return Ok(json!({ "synced": false, "reason": "no ticket" }));
        };
        let Ok(ticket) = raw.trim().parse::<ShareTicket>() else {
            eprintln!("share sync {share_id}: bad ticket");
            return Ok(json!({ "synced": false, "reason": "bad ticket" }));
        };
        let Some(node) = share.node().await else {
            eprintln!("share sync {share_id}: p2p offline");
            return Ok(json!({ "synced": false, "reason": "p2p offline" }));
        };
        tokio::time::timeout(
            crate::route::share::SHARE_NET_TIMEOUT,
            node.fetch(&ticket, &dir),
        )
        .await
        .map_err(|_| ApiError::new(504, "share sync fetch timed out"))??;
        if state
            .with_conn(|c| project_svc::find_by_share_id(c, share_id))?
            .is_some()
        {
            import_received(state, &dir, share_id)?;
        }
        touch(&reader_doc);
        return Ok(json!({ "synced": true, "role": "reader" }));
    }

    Err(ApiError::new(404, format!("share {share_id:?} not found")))
}

/// Share ids with a doc file directly under `dir` (published or received set,
/// depending on which dir is passed).
pub fn doc_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("automerge") {
                return None;
            }
            p.file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| valid_share_id(s))
                .map(String::from)
        })
        .collect()
}

/// 5-minute best-effort sync loop over every share, sequential, log-and-continue.
/// The task dies with the process.
pub fn spawn_interval_sync(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<AppState>();
            let share = app.state::<ShareState>();
            let dir = share.share_dir().to_path_buf();
            // Dedupe ids; sync_share errors on a same-id doc in both dirs.
            let mut ids: std::collections::BTreeSet<String> = doc_ids(&dir).into_iter().collect();
            ids.extend(doc_ids(&received_dir(&dir)));
            for id in ids {
                if let Err(e) = sync_share(&state, &share, &id).await {
                    eprintln!("share interval sync {id}: {} {}", e.status, e.detail);
                }
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use linxiv_core::storage;
    use tempfile::TempDir;

    fn mem_state() -> AppState {
        let conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        AppState::from_parts(conn, std::env::temp_dir(), std::env::temp_dir())
    }

    const SID: &str = "test_share_id";

    #[tokio::test]
    async fn paused_share_reports_paused() {
        let tmp = TempDir::new().unwrap();
        let state = mem_state();
        let share = ShareState::new(tmp.path());
        let settings = ShareSettings {
            paused: true,
            ..Default::default()
        };
        save_settings(tmp.path(), SID, &settings).unwrap();

        let v = sync_share(&state, &share, SID).await.unwrap();
        assert_eq!(v, json!({ "synced": false, "reason": "paused" }));
    }

    #[tokio::test]
    async fn project_gone_keeps_doc_and_settings_files() {
        let tmp = TempDir::new().unwrap();
        let state = mem_state();
        let share = ShareState::new(tmp.path());
        let doc = doc_path(tmp.path(), SID);
        fs::write(&doc, "dummy").unwrap();
        save_settings(tmp.path(), SID, &ShareSettings::default()).unwrap();

        let v = sync_share(&state, &share, SID).await.unwrap();
        assert_eq!(v, json!({ "synced": false, "reason": "project gone" }));
        assert!(doc.exists());
        assert!(settings_path(tmp.path(), SID).exists());
    }

    #[tokio::test]
    async fn hoster_shared_to_local_reports_direction() {
        let tmp = TempDir::new().unwrap();
        let state = mem_state();
        let share = ShareState::new(tmp.path());
        fs::write(doc_path(tmp.path(), SID), "dummy").unwrap();
        let settings = ShareSettings {
            paused: false,
            direction: SyncDirection::SharedToLocal,
        };
        save_settings(tmp.path(), SID, &settings).unwrap();

        let v = sync_share(&state, &share, SID).await.unwrap();
        assert_eq!(
            v,
            json!({ "synced": false, "reason": "direction", "role": "hoster" })
        );
    }

    #[tokio::test]
    async fn reader_missing_ticket_reports_no_ticket() {
        let tmp = TempDir::new().unwrap();
        let state = mem_state();
        let share = ShareState::new(tmp.path());
        let doc = doc_path(&received_dir(tmp.path()), SID);
        fs::create_dir_all(doc.parent().unwrap()).unwrap();
        fs::write(&doc, "dummy").unwrap();

        let v = sync_share(&state, &share, SID).await.unwrap();
        assert_eq!(v, json!({ "synced": false, "reason": "no ticket" }));
    }

    #[test]
    fn settings_default_unpaused_two_way() {
        let tmp = TempDir::new().unwrap();
        let settings = load_settings(tmp.path(), "nonexistent");
        assert!(!settings.paused);
        assert_eq!(settings.direction, SyncDirection::TwoWay);
    }
}

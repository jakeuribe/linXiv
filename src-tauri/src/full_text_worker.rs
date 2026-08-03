//! Background full-text indexer. While `full_text_worker_enabled` is on, it
//! walks the same backfill work list as `paper index-sources` (papers with no
//! TeX source yet, oldest first) one paper at a time, forever.
//!
//! Off by default: arXiv paces requests 7 s apart and tarballs run to megabytes,
//! so indexing a whole library is something the user opts into. The pacing
//! itself is not this module's job — every arXiv GET goes through
//! `sources::http`, which enforces the shared 7 s spacing and the 429 cool-down
//! across the whole process; the gap below is just extra politeness on top.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::Value;
use tauri::Manager;

use linxiv_core::config::UserSettings;
use linxiv_core::models::PaperDetails;
use linxiv_core::service::paper as svc_paper;
use rusqlite::Connection;

use crate::route::papers::{ingest_full_text, sid_key};
use crate::state::AppState;

const SETTING: &str = "full_text_worker_enabled";
/// Re-check when the worker is off, or the work list is empty.
const IDLE: Duration = Duration::from_secs(60);
/// Gap between two papers, on top of the 7 s the HTTP layer already enforces.
/// `ponytail: fixed gap; make it a setting if anyone wants to tune the pace.`
const GAP: Duration = Duration::from_secs(15);

/// Spawn the worker for the life of the app. Reads the setting every pass, so
/// toggling it in Settings takes effect without a restart.
pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Papers parked for this session: a fetch that fails (non-arXiv paper,
        // network error, corrupt tarball) leaves DOWNLOADED_SOURCE unset, so
        // without this the worker would retry the head of the list forever and
        // never reach the rest. Cleared on restart.
        let mut parked: HashSet<String> = HashSet::new();
        loop {
            if !enabled() {
                tokio::time::sleep(IDLE).await;
                continue;
            }
            let state = app.state::<AppState>();
            let Some(paper) = state.with_conn(|conn| next_candidate(conn, &parked)) else {
                tokio::time::sleep(IDLE).await;
                continue;
            };
            match ingest_full_text(&state, &paper).await {
                Ok(Some(chars)) => eprintln!("[full-text] {} — {chars} chars", paper.source_id),
                Ok(None) => {}
                Err(e) => {
                    parked.insert(paper.source_id.clone());
                    eprintln!(
                        "[full-text] {} skipped: {} {}",
                        paper.source_id, e.status, e.detail
                    );
                }
            }
            tokio::time::sleep(GAP).await;
        }
    });
}

/// Whether the setting is on. Loaded from disk each call (the settings file is
/// the only channel the UI has to reach this task).
fn enabled() -> bool {
    UserSettings::load()
        .ok()
        .and_then(|s| s.get(SETTING).and_then(Value::as_bool))
        .unwrap_or(false)
}

/// First un-parked paper on the backfill work list that still resolves.
fn next_candidate(conn: &Connection, parked: &HashSet<String>) -> Option<PaperDetails> {
    svc_paper::full_text_backfill_candidates(conn)
        .ok()?
        .into_iter()
        .filter(|id| !parked.contains(id))
        .find_map(|id| svc_paper::get(conn, &sid_key(&id)).ok().flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use linxiv_core::models::PaperMetadata;
    use linxiv_core::storage;
    use serde_json::json;

    fn paper(conn: &mut Connection, source_id: &str) {
        let meta: PaperMetadata = serde_json::from_value(json!({
            "source_id": source_id,
            "version": 1,
            "title": "T",
            "authors": ["Alice"],
            "published": "2024-01-01",
            "summary": "s",
            "category": "cs.LG",
            "url": format!("https://arxiv.org/pdf/{source_id}"),
            "source": "arxiv",
        }))
        .unwrap();
        svc_paper::save_paper_metadata(conn, &meta, None).unwrap();
    }

    #[test]
    fn parked_papers_are_skipped_and_indexed_ones_drop_off() {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        paper(&mut conn, "arxiv:a");
        paper(&mut conn, "arxiv:b");

        let mut parked = HashSet::new();
        assert_eq!(next_candidate(&conn, &parked).unwrap().source_id, "arxiv:a");

        parked.insert("arxiv:a".to_string());
        assert_eq!(next_candidate(&conn, &parked).unwrap().source_id, "arxiv:b");

        // set_full_text flips DOWNLOADED_SOURCE, so b leaves the work list too.
        svc_paper::set_full_text(&mut conn, "arxiv:b", 1, "body").unwrap();
        assert!(next_candidate(&conn, &parked).is_none());
    }
}

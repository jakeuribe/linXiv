//! Background full-text indexer. While `full_text_worker_enabled` is on, it
//! walks the same backfill work list as `paper index-sources` (papers with no
//! TeX source yet, oldest first) one paper at a time, forever.
//!
//! Off by default: arXiv paces requests 7 s apart and tarballs run to megabytes,
//! so indexing a whole library is something the user opts into. Every arXiv GET
//! made here goes through `sources::http`, which serialises requests behind the
//! shared 7 s spacing and the 429 cool-down; `GAP` is an additional wait this
//! module imposes between papers.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::Manager;

use linxiv_core::config::UserSettings;
use linxiv_core::models::PaperDetails;
use linxiv_core::service::paper as svc_paper;
use rusqlite::Connection;

use crate::route::papers::{ingest_full_text, sid_key};
use crate::state::AppState;

const SETTING: &str = "full_text_worker_enabled";
/// Poll cadence while the worker is switched off — a small JSON file read.
const OFF_POLL: Duration = Duration::from_secs(20);
/// Wait before rebuilding the work list when nothing is currently eligible.
/// Longer than `OFF_POLL` because this one costs a DB scan under the shared
/// connection lock, and it is the steady state once a library is fully indexed.
const IDLE: Duration = Duration::from_secs(300);
/// Gap between two papers, on top of the 7 s the HTTP layer already enforces.
/// `ponytail: fixed gap; make it a setting if anyone wants to tune the pace.`
const GAP: Duration = Duration::from_secs(15);
/// How long a paper that failed to index is left alone before another attempt.
const RETRY_AFTER: Duration = Duration::from_secs(600);

/// When a parked paper may be tried again; `None` means never (this session).
type ParkedUntil = Option<Instant>;

/// Spawn the worker for the life of the app. Reads the setting every pass, so
/// toggling it in Settings takes effect without a restart.
pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // A failed fetch leaves DOWNLOADED_SOURCE unset, so without parking the
        // loop would retry the head of the list forever and never reach the
        // rest. Papers with no arXiv source to fetch are parked for the session;
        // everything else (offline, 429, corrupt tarball) comes back after
        // RETRY_AFTER, so an evening with no network doesn't turn the whole
        // backlog into a permanent skip list.
        let mut parked: HashMap<String, ParkedUntil> = HashMap::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        loop {
            if !enabled() {
                tokio::time::sleep(OFF_POLL).await;
                continue;
            }
            let state = app.state::<AppState>();
            let next = state.with_conn(|conn| {
                if queue.is_empty() {
                    refill(conn, &mut queue, &mut parked, Instant::now());
                }
                take_next(conn, &mut queue, &parked)
            });
            let Some(paper) = next else {
                tokio::time::sleep(IDLE).await;
                continue;
            };
            // Non-arXiv papers never leave the work list, and the check is
            // free — do it before spending GAP or a request on them.
            if let Err(e) = svc_paper::source_fetch_url(&paper) {
                eprintln!("[full-text] {} parked: {e}", paper.source_id);
                parked.insert(paper.source_id, None);
                continue;
            }
            let retry_at = Instant::now() + RETRY_AFTER;
            match ingest_full_text(&state, &paper).await {
                Ok(Some(0)) => eprintln!(
                    "[full-text] {} — tarball held no TeX, indexed empty",
                    paper.source_id
                ),
                Ok(Some(chars)) => eprintln!("[full-text] {} — {chars} chars", paper.source_id),
                // Nothing was written, so the paper is still on the work list.
                // Parking it keeps the loop off a re-download treadmill.
                Ok(None) => {
                    parked.insert(paper.source_id, Some(retry_at));
                }
                Err(e) => {
                    eprintln!(
                        "[full-text] {} failed, retrying later: {} {}",
                        paper.source_id, e.status, e.detail
                    );
                    parked.insert(paper.source_id, Some(retry_at));
                }
            }
            tokio::time::sleep(GAP).await;
        }
    });
}

/// Whether the setting is on. Loaded from disk each call (the settings file is
/// the only channel the UI has to reach this task). Unreadable settings hold
/// the worker off rather than starting network activity on a guess.
fn enabled() -> bool {
    match UserSettings::load() {
        Ok(s) => s.get(SETTING).and_then(Value::as_bool).unwrap_or(false),
        Err(e) => {
            eprintln!("[full-text] settings unreadable, staying idle: {e}");
            false
        }
    }
}

/// Rebuild the work list, dropping park entries whose retry time has passed so
/// those papers become eligible again.
fn refill(
    conn: &Connection,
    queue: &mut VecDeque<String>,
    parked: &mut HashMap<String, ParkedUntil>,
    now: Instant,
) {
    parked.retain(|_, until| until.is_none_or(|t| t > now));
    match svc_paper::full_text_backfill_candidates(conn) {
        Ok(ids) => queue.extend(ids),
        Err(e) => eprintln!("[full-text] work list unavailable: {e}"),
    }
}

/// Pop ids until one is un-parked and still resolves to a paper. Ids that fail
/// either check are consumed, so the caller's next pass moves on.
fn take_next(
    conn: &Connection,
    queue: &mut VecDeque<String>,
    parked: &HashMap<String, ParkedUntil>,
) -> Option<PaperDetails> {
    while let Some(id) = queue.pop_front() {
        if parked.contains_key(&id) {
            continue;
        }
        if let Some(paper) = svc_paper::get(conn, &sid_key(&id)).ok().flatten() {
            return Some(paper);
        }
    }
    None
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

    /// Drain the whole work list once, from a fresh queue.
    fn drain(
        conn: &Connection,
        parked: &mut HashMap<String, ParkedUntil>,
        now: Instant,
    ) -> Vec<String> {
        let mut queue = VecDeque::new();
        refill(conn, &mut queue, parked, now);
        let mut seen = Vec::new();
        while let Some(p) = take_next(conn, &mut queue, parked) {
            seen.push(p.source_id);
        }
        seen
    }

    #[test]
    fn parked_papers_are_skipped_and_indexed_ones_drop_off() {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        paper(&mut conn, "arxiv:a");
        paper(&mut conn, "arxiv:b");
        let now = Instant::now();

        let mut parked = HashMap::new();
        assert_eq!(drain(&conn, &mut parked, now), ["arxiv:a", "arxiv:b"]);

        parked.insert("arxiv:a".to_string(), None);
        assert_eq!(drain(&conn, &mut parked, now), ["arxiv:b"]);

        // set_full_text flips DOWNLOADED_SOURCE, so b leaves the work list too.
        svc_paper::set_full_text(&mut conn, "arxiv:b", 1, "body").unwrap();
        assert!(drain(&conn, &mut parked, now).is_empty());
    }

    #[test]
    fn a_transient_park_expires_but_a_permanent_one_does_not() {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        paper(&mut conn, "arxiv:offline");
        paper(&mut conn, "arxiv:notarxiv");
        let now = Instant::now();

        let mut parked = HashMap::from([
            ("arxiv:offline".to_string(), Some(now + RETRY_AFTER)),
            ("arxiv:notarxiv".to_string(), None),
        ]);
        assert!(drain(&conn, &mut parked, now).is_empty());

        // Past the retry time, only the transiently parked paper comes back —
        // the one with nothing to fetch stays parked for the session.
        let later = now + RETRY_AFTER + Duration::from_secs(1);
        assert_eq!(drain(&conn, &mut parked, later), ["arxiv:offline"]);
    }
}

//! Background full-text indexer. While `full_text_worker_enabled` is on, it
//! walks the same backfill work list as `paper index-sources` (papers with no
//! TeX source yet, oldest first) one paper at a time, forever.
//!
//! Off by default: arXiv paces requests 7 s apart and tarballs run to megabytes,
//! so indexing a whole library is something the user opts into. Every arXiv GET
//! made here goes through `sources::http`, which serialises requests behind the
//! shared 7 s spacing and the 429 cool-down; `GAP` is an additional wait this
//! module imposes between papers.
//!
//! `ponytail: no cap on how much TeX this stores. A body averages ~150 KB and is
//! capped at MAX_TEX_BYTES (16 MiB) per paper. papers_fts is a plain fts5 table,
//! so it keeps its own copy plus an index: budget ~3x the raw text, i.e. north of
//! a gigabyte for a 3000-paper library. Add a full_text_save_limit_mb setting,
//! like pdf_save_limit_mb, if that bites.`

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
const OFF_POLL: Duration = Duration::from_secs(60);
/// Ceiling on the wait before rebuilding the work list when nothing is eligible.
/// Longer than `OFF_POLL` because this one costs a DB scan under the shared
/// connection lock, and it is the steady state once a library is fully indexed.
const IDLE: Duration = Duration::from_secs(300);
/// Gap between two papers, on top of the 7 s the HTTP layer already enforces.
/// `ponytail: fixed gap; make it a setting if anyone wants to tune the pace.`
const GAP: Duration = Duration::from_secs(15);
/// How long a paper that failed to index is left alone before another attempt.
const RETRY_AFTER: Duration = Duration::from_secs(600);
/// Failures after which a paper is left alone until the app restarts.
const MAX_ATTEMPTS: u32 = 5;
/// Ceiling on the consecutive-failure backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(3600);
/// Wait before restarting the loop after it panics, and how often to try.
const RESTART_DELAY: Duration = Duration::from_secs(60);
const MAX_RESTARTS: u32 = 5;

/// A paper the loop is holding off on. `until: None` means "not again this
/// session" — either there is no arXiv source to fetch, or it has failed
/// `MAX_ATTEMPTS` times.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Park {
    attempts: u32,
    until: Option<Instant>,
}

impl Park {
    const PERMANENT: Park = Park {
        attempts: MAX_ATTEMPTS,
        until: None,
    };
}

/// Spawn the worker for the life of the app, restarting it if it panics —
/// otherwise the feature would end silently with the toggle still reading "on".
///
/// Bounded, because the likeliest panic is `with_conn` on a poisoned DB mutex,
/// which stays poisoned for the process: restarting into it forever would just
/// print a panic a minute.
pub fn spawn(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        for attempt in 1..=MAX_RESTARTS {
            let task = tauri::async_runtime::spawn(run(app.clone()));
            match task.await {
                Ok(()) => return,
                Err(e) => eprintln!("[full-text] worker stopped ({attempt}/{MAX_RESTARTS}): {e}"),
            }
            tokio::time::sleep(RESTART_DELAY).await;
        }
        eprintln!("[full-text] worker kept failing; not restarting again this session");
    });
}

/// Sleep, but in `OFF_POLL` steps, giving up the rest as soon as the setting is
/// switched off. The post-failure backoff runs to `MAX_BACKOFF`, so a single
/// `sleep` would leave the toggle looking dead for up to an hour after a night
/// offline.
async fn nap(total: Duration) {
    let mut slept = Duration::ZERO;
    while slept < total {
        let step = OFF_POLL.min(total - slept);
        tokio::time::sleep(step).await;
        if !enabled() {
            return;
        }
        slept += step;
    }
}

/// The loop itself. Reads the setting at the top of each pass and during long
/// waits, so switching it off takes effect within `OFF_POLL` and switching it on
/// within `OFF_POLL` of the current wait ending.
async fn run(app: tauri::AppHandle) {
    // A failed fetch leaves DOWNLOADED_SOURCE unset, so without parking the loop
    // would retry the head of the list forever and never reach the rest.
    let mut parked: HashMap<String, Park> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    // Failures in a row across different papers. A global condition (offline,
    // arXiv rate-limiting the whole IP) fails every paper it touches, so backing
    // off on this rather than per-paper is what stops the loop from spending all
    // day making doomed requests.
    let mut consecutive_failures: u32 = 0;
    loop {
        if !enabled() {
            tokio::time::sleep(OFF_POLL).await;
            continue;
        }
        let state = app.state::<AppState>();
        let now = Instant::now();
        let next = state.with_conn(|conn| {
            if queue.is_empty() {
                refill(conn, &mut queue);
            }
            take_next(conn, &mut queue, &parked, now)
        });
        let Some(paper) = next else {
            nap(idle_wait(&parked, Instant::now())).await;
            continue;
        };
        // The work-list query selects only papers source_fetch_url accepts, so
        // this rejects little; it is what keeps a query change from turning into
        // a request per unfetchable paper.
        if let Err(e) = svc_paper::source_fetch_url(&paper) {
            eprintln!("[full-text] {} parked: {e}", paper.source_id);
            parked.insert(paper.source_id, Park::PERMANENT);
            // Nothing else on this path awaits, and it runs once per paper the
            // two rules disagree about.
            tokio::task::yield_now().await;
            continue;
        }
        let sid = paper.source_id.clone();
        match ingest_full_text(&state, &paper).await {
            Ok(indexed) => {
                match indexed {
                    Some(0) => eprintln!("[full-text] {sid} — tarball held no TeX, indexed empty"),
                    Some(chars) => eprintln!("[full-text] {sid} — {chars} chars"),
                    // Nothing was written, so DOWNLOADED_SOURCE is still unset
                    // and the paper is still a candidate — park it rather than
                    // fetch the same tarball again next pass.
                    None => eprintln!("[full-text] {sid} — empty re-fetch, kept the stored text"),
                }
                consecutive_failures = 0;
                match indexed {
                    Some(_) => parked.remove(&sid),
                    None => parked.insert(sid, Park::PERMANENT),
                };
                nap(GAP).await;
            }
            Err(e) => {
                eprintln!("[full-text] {sid} failed: {} {}", e.status, e.detail);
                // Only a failure that stands alone counts against this paper.
                // During an outage every paper fails, and burning one attempt
                // each would drop healthy papers for the rest of the session.
                let isolated = consecutive_failures == 0;
                let park = park_after_failure(parked.get(&sid), Instant::now(), isolated);
                parked.insert(sid, park);
                consecutive_failures = consecutive_failures.saturating_add(1);
                nap(backoff(consecutive_failures)).await;
            }
        }
    }
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

/// Park state after another failure: try again in `RETRY_AFTER`, until the paper
/// has burned `MAX_ATTEMPTS` — a `/src/` URL that 404s (withdrawn submissions)
/// would otherwise be re-fetched every 10 minutes forever. `isolated` is false
/// when other papers are failing too, which counts against the run rather than
/// against this paper.
fn park_after_failure(prev: Option<&Park>, now: Instant, isolated: bool) -> Park {
    let prev_attempts = prev.map_or(0, |p| p.attempts);
    let attempts = if isolated {
        prev_attempts.saturating_add(1)
    } else {
        prev_attempts
    };
    Park {
        attempts,
        until: (attempts < MAX_ATTEMPTS).then(|| now + RETRY_AFTER),
    }
}

/// Wait after `n` failures in a row: `GAP` doubled per failure, capped.
fn backoff(n: u32) -> Duration {
    GAP.saturating_mul(1u32 << n.min(8)).min(MAX_BACKOFF)
}

/// Wait before rebuilding the work list: no longer than the nearest park expiry,
/// so a paper due back in 10 s isn't held for the full `IDLE`.
///
/// Deadlines already past are ignored. A park entry outlives its paper — the
/// user can index it by hand, or delete it — and `saturating_duration_since`
/// reports such a deadline as zero, which would spin the idle path into a scan
/// loop with no sleep in it.
fn idle_wait(parked: &HashMap<String, Park>, now: Instant) -> Duration {
    parked
        .values()
        .filter_map(|p| p.until)
        .filter(|&t| t > now)
        .map(|t| t.saturating_duration_since(now))
        .min()
        .unwrap_or(IDLE)
        .min(IDLE)
}

/// Rebuild the work list. Park entries survive: they carry the attempt count,
/// and expiry is applied when a paper is taken, not here.
fn refill(conn: &Connection, queue: &mut VecDeque<String>) {
    match svc_paper::full_text_backfill_candidates(conn) {
        Ok(ids) => queue.extend(ids),
        Err(e) => eprintln!("[full-text] work list unavailable: {e}"),
    }
}

/// Pop ids until one is due and still resolves to an unindexed paper. Ids that
/// fail a check are consumed; the next `refill` puts back any that still qualify.
///
/// The queue is rebuilt only when it empties, which for a large library is hours
/// away, so `downloaded_source` is re-read here: the manual button, the MCP tool
/// and the CLI all index papers this list already holds.
fn take_next(
    conn: &Connection,
    queue: &mut VecDeque<String>,
    parked: &HashMap<String, Park>,
    now: Instant,
) -> Option<PaperDetails> {
    while let Some(id) = queue.pop_front() {
        if !is_due(parked.get(&id), now) {
            continue;
        }
        match svc_paper::get(conn, &sid_key(&id)) {
            Ok(Some(paper)) if !paper.downloaded_source => return Some(paper),
            Ok(_) => {} // deleted, or indexed by another path since the rebuild
            Err(e) => eprintln!("[full-text] {id} unreadable: {e}"),
        }
    }
    None
}

/// Whether a paper may be tried now: never parked, or past its retry time.
fn is_due(park: Option<&Park>, now: Instant) -> bool {
    park.is_none_or(|p| p.until.is_some_and(|t| t <= now))
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
    fn drain(conn: &Connection, parked: &HashMap<String, Park>, now: Instant) -> Vec<String> {
        let mut queue = VecDeque::new();
        refill(conn, &mut queue);
        let mut seen = Vec::new();
        while let Some(p) = take_next(conn, &mut queue, parked, now) {
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
        assert_eq!(drain(&conn, &parked, now), ["arxiv:a", "arxiv:b"]);

        parked.insert("arxiv:a".to_string(), Park::PERMANENT);
        assert_eq!(drain(&conn, &parked, now), ["arxiv:b"]);

        // set_full_text flips DOWNLOADED_SOURCE, so b leaves the work list too.
        svc_paper::set_full_text(&mut conn, "arxiv:b", 1, "body").unwrap();
        assert!(drain(&conn, &parked, now).is_empty());
    }

    #[test]
    fn a_transient_park_expires_but_a_permanent_one_does_not() {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        paper(&mut conn, "arxiv:offline");
        paper(&mut conn, "arxiv:notarxiv");
        let now = Instant::now();

        let parked = HashMap::from([
            (
                "arxiv:offline".to_string(),
                park_after_failure(None, now, true),
            ),
            ("arxiv:notarxiv".to_string(), Park::PERMANENT),
        ]);
        assert!(drain(&conn, &parked, now).is_empty());

        // Past the retry time, only the transiently parked paper comes back.
        let later = now + RETRY_AFTER + Duration::from_secs(1);
        assert_eq!(drain(&conn, &parked, later), ["arxiv:offline"]);
    }

    #[test]
    fn a_paper_that_keeps_failing_is_given_up_on() {
        let now = Instant::now();
        let mut park = park_after_failure(None, now, true);
        for _ in 1..MAX_ATTEMPTS {
            assert!(
                park.until.is_some(),
                "still retrying at {} attempts",
                park.attempts
            );
            park = park_after_failure(Some(&park), now, true);
        }
        assert_eq!(park.attempts, MAX_ATTEMPTS);
        assert_eq!(park.until, None);
        // Past its would-be retry time it is still not due.
        assert!(!is_due(Some(&park), now + RETRY_AFTER * 100));
    }

    #[test]
    fn an_outage_does_not_spend_a_papers_attempts() {
        // Every paper fails while the network is down. If those counted, a night
        // offline would retire the whole library until the app restarts.
        let now = Instant::now();
        let mut park = park_after_failure(None, now, false);
        for _ in 0..50 {
            park = park_after_failure(Some(&park), now, false);
        }
        assert_eq!(park.attempts, 0);
        assert_eq!(park.until, Some(now + RETRY_AFTER));
        assert!(is_due(Some(&park), now + RETRY_AFTER));
    }

    #[test]
    fn an_indexed_paper_left_on_a_stale_queue_is_skipped() {
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        paper(&mut conn, "arxiv:a");
        paper(&mut conn, "arxiv:b");
        let mut queue = VecDeque::new();
        refill(&conn, &mut queue);

        // The manual button / MCP / CLI indexes a paper the queue already holds.
        svc_paper::set_full_text(&mut conn, "arxiv:a", 1, "indexed elsewhere").unwrap();

        let parked = HashMap::new();
        let taken = take_next(&conn, &mut queue, &parked, Instant::now());
        assert_eq!(taken.unwrap().source_id, "arxiv:b");
    }

    #[test]
    fn consecutive_failures_back_off_up_to_a_cap() {
        assert_eq!(backoff(1), GAP * 2);
        assert_eq!(backoff(4), GAP * 16);
        // An outage that fails every paper stops hammering within an hour-long wait.
        assert_eq!(backoff(20), MAX_BACKOFF);
    }

    #[test]
    fn idle_wait_tracks_the_nearest_retry() {
        let now = Instant::now();
        assert_eq!(idle_wait(&HashMap::new(), now), IDLE);

        let soon = Duration::from_secs(10);
        let parked = HashMap::from([
            ("a".to_string(), Park::PERMANENT),
            (
                "b".to_string(),
                Park {
                    attempts: 1,
                    until: Some(now + soon),
                },
            ),
        ]);
        assert_eq!(idle_wait(&parked, now), soon);
        // A deadline in the past belongs to a paper that left the work list.
        // Counting it would mean sleeping zero and rescanning the DB flat out.
        let stale = HashMap::from([(
            "d".to_string(),
            Park {
                attempts: 1,
                until: Some(now - Duration::from_secs(1)),
            },
        )]);
        assert_eq!(idle_wait(&stale, now), IDLE);
        // Never longer than IDLE, even with every retry far out.
        let far = HashMap::from([(
            "c".to_string(),
            Park {
                attempts: 1,
                until: Some(now + IDLE * 10),
            },
        )]);
        assert_eq!(idle_wait(&far, now), IDLE);
    }
}

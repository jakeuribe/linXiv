//! Two-way share sync glue: per-share settings sidecars, received→canonical
//! import, and the role-aware `sync_share` shared by the route arm and the interval task.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use linxiv_core::service::paper as paper_svc;
use linxiv_core::service::project as project_svc;
use linxiv_share::{
    apply_removals, build_shared_project, doc_path, e2ee_dir, e2ee_received_dir,
    import_shared_project, load, received_dir, save, valid_share_id, ShareNode, ShareTicket,
    SharedProject,
};

use crate::route::share::ShareState;
use crate::route::{to_value, ApiError};
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum SyncDirection {
    #[default]
    TwoWay,
    SharedToLocal,
    LocalToShared,
}

/// Per-share sync settings, stored as `share_dir/settings/<id>.json` — share-local
/// sidecar state.
#[derive(Debug, Clone, Serialize, Deserialize, Default, ts_rs::TS)]
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
    let sp = match ShareNode::received(share_dir, share_id) {
        Err(linxiv_share::ShareError::NotFound(_)) => {
            ShareNode::e2ee_received(share_dir, share_id)?
        }
        other => other?,
    };
    Ok(state.with_conn(|conn| import_shared_project(conn, &sp))?)
}

/// Fill `pdf_blob` tickets before an e2ee publish: tickets already in the
/// published doc carry forward by source_id; a paper with a local PDF but no
/// ticket gets one stored. `rekey` skips the carry-forward and re-encrypts
/// every blob under the new epoch. Blob failures are logged and skipped.
pub(crate) async fn populate_pdf_blobs(
    state: &AppState,
    node: &ShareNode,
    share_dir: &Path,
    sp: &mut SharedProject,
    rekey: bool,
) -> Result<(), linxiv_share::ShareError> {
    // Prior doc loads even on rekey: its tickets are the re-encrypt source for
    // papers with no local PDF.
    let prior = match load(&e2ee_dir(share_dir), &sp.share_id) {
        Ok(d) => Some(d),
        // NotFound = no prior doc (first publish); other load errors abort the pass.
        Err(linxiv_share::ShareError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let prior_blobs: std::collections::HashMap<(&str, i64), &str> = prior
        .as_ref()
        .map(|d| {
            d.papers
                .iter()
                .filter_map(|q| Some(((q.source_id.as_str(), q.version), q.pdf_blob.as_deref()?)))
                .collect()
        })
        .unwrap_or_default();
    for p in &mut sp.papers {
        let prior_ticket = prior_blobs
            .get(&(p.source_id.as_str(), p.version))
            .map(|s| s.to_string());
        if !rekey {
            p.pdf_blob = prior_ticket.clone();
            if p.pdf_blob.is_some() {
                continue;
            }
        }
        let custom = match state
            .with_conn(|c| paper_svc::pdf_custom_path(c, &p.source_id, Some(p.version)))
        {
            Ok(path) => path,
            Err(e) => {
                eprintln!(
                    "share {}: paper lookup for {}: {e}",
                    sp.share_id, p.source_id
                );
                None
            }
        };
        let Some(path) = linxiv_core::service::files::pdf_path(
            &state.pdf_dir,
            &p.source_id,
            p.version,
            custom.as_deref(),
        ) else {
            // Rekey with no local PDF: re-encrypt from the prior blob under the
            // new epoch; on failure keep the old ticket.
            if rekey {
                if let Some(old) = prior_ticket {
                    // Read falls back to a network fetch when the blob store lost
                    // the blob; cap the round trip with the e2ee net budget.
                    let reencrypted = match tokio::time::timeout(
                        crate::route::share::SHARE_NET_TIMEOUT * 2,
                        async {
                            let bytes = node.read_pdf_blob(&sp.share_id, &old, u64::MAX).await?;
                            node.store_pdf_blob(&sp.share_id, &bytes).await
                        },
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => Err(linxiv_share::ShareError::Transport(
                            "rekey blob read timed out".into(),
                        )),
                    };
                    match reencrypted {
                        Ok(ticket) => p.pdf_blob = Some(ticket),
                        Err(e) => {
                            eprintln!(
                                "share {}: rekey blob for {}: {e}; keeping old-epoch ticket",
                                sp.share_id, p.source_id
                            );
                            p.pdf_blob = Some(old);
                        }
                    }
                }
            }
            continue;
        };
        let read_path = path.clone();
        let bytes = match tokio::task::spawn_blocking(move || std::fs::read(&read_path)).await {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                eprintln!("share {}: PDF read {}: {e}", sp.share_id, path.display());
                continue;
            }
            Err(e) => {
                eprintln!(
                    "share {}: PDF read task {}: {e}",
                    sp.share_id,
                    path.display()
                );
                continue;
            }
        };
        match node.store_pdf_blob(&sp.share_id, &bytes).await {
            Ok(ticket) => p.pdf_blob = Some(ticket),
            Err(e) => eprintln!("share {}: pdf blob for {}: {e}", sp.share_id, p.source_id),
        }
    }
    Ok(())
}

/// The applied-baseline sidecar dir for a mirror dir (`<mirror>/applied/`).
pub(crate) fn applied_dir(mirror_dir: &Path) -> PathBuf {
    mirror_dir.join("applied")
}

/// Advance the deletion-propagation baseline to the just-applied mirror state.
/// Only called after a successful import (+ removal) pass; best-effort — a
/// failed copy just re-propagates from the older baseline next pass.
fn advance_applied(mirror_dir: &Path, share_id: &str) {
    let dst_dir = applied_dir(mirror_dir);
    if let Err(e) = std::fs::create_dir_all(&dst_dir).and_then(|()| {
        std::fs::copy(doc_path(mirror_dir, share_id), doc_path(&dst_dir, share_id)).map(|_| ())
    }) {
        eprintln!("share sync {share_id}: applied-baseline copy failed: {e}");
    }
}

/// Apply remote deletions (prior mirror − fresh mirror) to the linked project,
/// after the additive import. Log-and-report; a logged line per non-empty pass.
fn propagate_removals(
    state: &AppState,
    share_id: &str,
    prior: &SharedProject,
    fresh: &SharedProject,
    project_fk: i64,
) -> Result<(), ApiError> {
    let removed = state.with_conn(|c| apply_removals(c, prior, fresh, Some(project_fk)))?;
    if !removed.is_empty() {
        println!(
            "share sync {share_id}: propagated remote deletions papers={} notes={} annotations={} tags={}",
            removed.papers, removed.notes, removed.annotations, removed.tags,
        );
    }
    Ok(())
}

/// Bump mtime — the UI reads the doc file's mtime as synced_at.
fn touch(p: &Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// `POST /api/share/{id}/sync` `role` — which sync leg ran.
#[derive(Debug, Clone, Copy, Serialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
pub enum SyncRole {
    Hoster,
    Reader,
}

/// `POST /api/share/{id}/sync` `reason` — why a pass skipped or came up short.
#[derive(Debug, Clone, Copy, Serialize, ts_rs::TS)]
pub enum SyncReason {
    #[serde(rename = "paused")]
    Paused,
    #[serde(rename = "direction")]
    Direction,
    #[serde(rename = "project gone")]
    ProjectGone,
    #[serde(rename = "no ticket")]
    NoTicket,
    #[serde(rename = "bad ticket")]
    BadTicket,
    #[serde(rename = "p2p offline")]
    P2pOffline,
    #[serde(rename = "awaiting first sync")]
    AwaitingFirstSync,
    #[serde(rename = "no key for any content")]
    NoKeyForAnyContent,
    #[serde(rename = "revoked or awaiting key")]
    RevokedOrAwaitingKey,
}

/// `POST /api/share/{id}/sync` response — pass skipped, `reason` says why.
#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SyncSkipped {
    #[ts(type = "false")]
    synced: bool,
    reason: SyncReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    role: Option<SyncRole>,
}

/// `POST /api/share/{id}/sync` response — pass completed; e2ee legs add counters.
#[derive(Debug, Serialize, ts_rs::TS)]
pub struct SyncedReceipt {
    #[ts(type = "true")]
    synced: bool,
    role: SyncRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    e2ee: Option<bool>,
    /// Hoster leg: devices this share is currently granted to.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    members: Option<usize>,
    /// Reader leg: commits decrypted and applied this pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    applied: Option<usize>,
    /// Reader leg: commits fetched with no key for their epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    no_key: Option<usize>,
    /// Reader leg: commits that failed to decrypt for any other reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    failed: Option<usize>,
    /// The sync ran but the mirror is still empty — the host has not answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    reason: Option<SyncReason>,
    /// Notes/annotations skipped: key revoked or not yet received.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    undecryptable: Option<usize>,
}

/// `{ synced: false, reason [, role] }` skip envelope.
fn skipped(reason: SyncReason, role: Option<SyncRole>) -> Result<Value, ApiError> {
    to_value(&SyncSkipped {
        synced: false,
        reason,
        role,
    })
}

/// Bare synced receipt; the e2ee legs fill their extras via struct update.
fn synced(role: SyncRole) -> SyncedReceipt {
    SyncedReceipt {
        synced: true,
        role,
        e2ee: None,
        members: None,
        applied: None,
        no_key: None,
        failed: None,
        pending: None,
        reason: None,
        undecryptable: None,
    }
}

/// One sync pass for one share, honoring role (hoster/reader, by which doc file
/// exists) and settings (paused + direction). Route arm + interval task both call this.
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
        return skipped(SyncReason::Paused, None);
    }
    let _guard = share.lock_writes(share_id).await;

    let hoster_doc = doc_path(&dir, share_id);
    let reader_doc = doc_path(&received_dir(&dir), share_id);
    let e2ee_hoster_doc = doc_path(&e2ee_dir(&dir), share_id);
    let e2ee_reader_doc = doc_path(&e2ee_received_dir(&dir), share_id);
    let roles = [&hoster_doc, &reader_doc, &e2ee_hoster_doc, &e2ee_reader_doc];
    if roles.iter().filter(|p| p.is_file()).count() > 1 {
        return Err(ApiError::new(
            500,
            format!("share {share_id} has doc files in more than one role"),
        ));
    }

    if hoster_doc.is_file() {
        // Hoster leg = local_to_shared: rebuild + save + re-register the doc.
        // Its shared_to_local leg is a no-op until W4 editors give readers edits.
        if settings.direction == SyncDirection::SharedToLocal {
            return skipped(SyncReason::Direction, Some(SyncRole::Hoster));
        }
        let Some(fk) = state.with_conn(|c| project_svc::find_by_share_id(c, share_id))? else {
            // Doc + settings stay on disk; only explicit unpublish deletes them.
            return skipped(SyncReason::ProjectGone, None);
        };
        let sp = state.with_conn(|c| build_shared_project(c, fk))?;
        let doc = save(&dir, &sp)?;
        if let Some(node) = share.node().await {
            node.register_doc(share_id, doc)?;
        }
        touch(&hoster_doc);
        return to_value(&synced(SyncRole::Hoster));
    }

    if reader_doc.is_file() {
        // Reader leg = shared_to_local: refetch from the stored ticket, then
        // import into the linked project.
        if settings.direction == SyncDirection::LocalToShared {
            return skipped(SyncReason::Direction, Some(SyncRole::Reader));
        }
        let Ok(raw) = std::fs::read_to_string(ticket_path(&dir, share_id)) else {
            eprintln!("share sync {share_id}: no ticket file");
            return skipped(SyncReason::NoTicket, None);
        };
        let Ok(ticket) = raw.trim().parse::<ShareTicket>() else {
            eprintln!("share sync {share_id}: bad ticket");
            return skipped(SyncReason::BadTicket, None);
        };
        let Some(node) = share.node().await else {
            eprintln!("share sync {share_id}: p2p offline");
            return skipped(SyncReason::P2pOffline, None);
        };
        // Deletion-propagation baseline: the last mirror state actually APPLIED
        // to the DB (sidecar under applied/), falling back to the pre-fetch
        // mirror before any sidecar exists. The mirror file itself is refreshed
        // by the fetch, so it can't be the baseline — a pass that dies between
        // fetch and apply would silently swallow the host's removals.
        let applied_dir = applied_dir(&received_dir(&dir));
        let prior = load(&applied_dir, share_id)
            .ok()
            .or_else(|| ShareNode::received(&dir, share_id).ok());
        tokio::time::timeout(
            crate::route::share::SHARE_NET_TIMEOUT,
            node.fetch(&ticket, &dir),
        )
        .await
        .map_err(|_| ApiError::new(504, "share sync fetch timed out"))??;
        if let Some(fk) = state.with_conn(|c| project_svc::find_by_share_id(c, share_id))? {
            import_received(state, &dir, share_id)?;
            if let Some(prior) = prior {
                let fresh = ShareNode::received(&dir, share_id)?;
                propagate_removals(state, share_id, &prior, &fresh, fk)?;
            }
            advance_applied(&received_dir(&dir), share_id);
        }
        touch(&reader_doc);
        return to_value(&synced(SyncRole::Reader));
    }

    if e2ee_hoster_doc.is_file() {
        // E2ee hoster leg: each cycle rebuilds from canonical SQLite and the
        // on-disk e2ee doc, then evolves the encrypted state (never dials).
        // Editor merges in the beelay doc are not hydrated back into SQLite;
        // TwoWay behaves as local_to_shared.
        if settings.direction == SyncDirection::SharedToLocal {
            return skipped(SyncReason::Direction, Some(SyncRole::Hoster));
        }
        let Some(fk) = state.with_conn(|c| project_svc::find_by_share_id(c, share_id))? else {
            // Doc + settings stay on disk; only explicit unpublish deletes them.
            return skipped(SyncReason::ProjectGone, None);
        };
        let mut sp = state.with_conn(|c| build_shared_project(c, fk))?;
        let Some(node) = share.node().await else {
            eprintln!("share sync {share_id}: p2p offline");
            return skipped(SyncReason::P2pOffline, None);
        };
        // ponytail: a failed publish below orphans just-stored blobs (random
        // nonce, no dedup) and nothing GCs them; upgrade: sweep unreferenced tickets.
        populate_pdf_blobs(state, &node, &dir, &mut sp, false).await?;
        tokio::time::timeout(
            crate::route::share::SHARE_NET_TIMEOUT * 2,
            node.publish_secure(&sp),
        )
        .await
        .map_err(|_| ApiError::new(504, "share sync publish timed out"))??;
        touch(&e2ee_hoster_doc);
        // Host-side counterpart of the reader line below: what this device just
        // republished, and to how many live members.
        let members = crate::route::share::live_member_count(&dir, share_id);
        println!(
            "share sync {share_id}: hoster republished papers={} notes={} annotations={} members={members}",
            sp.papers.len(),
            sp.notes.len(),
            sp.annotations.len(),
        );
        return to_value(&SyncedReceipt {
            e2ee: Some(true),
            members: Some(members),
            ..synced(SyncRole::Hoster)
        });
    }

    if e2ee_reader_doc.is_file() {
        // E2ee reader leg: dial the host, refresh the mirror, then import into
        // the linked project. A revoked device surfaces as sync_e2ee's NotFound.
        if settings.direction == SyncDirection::LocalToShared {
            return skipped(SyncReason::Direction, Some(SyncRole::Reader));
        }
        let Some(node) = share.node().await else {
            eprintln!("share sync {share_id}: p2p offline");
            return skipped(SyncReason::P2pOffline, None);
        };
        // Deletion-propagation baseline: last-applied sidecar, falling back to
        // the pre-sync mirror (same rationale as the plain reader leg).
        let prior = load(&applied_dir(&e2ee_received_dir(&dir)), share_id)
            .ok()
            .or_else(|| ShareNode::e2ee_received(&dir, share_id).ok());
        // Keyhive/BeeKEM ops run slower than plain sync; double the net budget.
        let outcome = tokio::time::timeout(
            crate::route::share::SHARE_NET_TIMEOUT * 2,
            node.sync_e2ee(share_id),
        )
        .await
        .map_err(|_| ApiError::new(504, "share sync timed out"))??;
        let linked = state.with_conn(|c| project_svc::find_by_share_id(c, share_id))?;
        // A mirror still empty after the sync (host asleep, or no key yet) has
        // nothing to import — and a manual retry needs told that, or it reads
        // as a silent success that changed nothing.
        let mut pending = false;
        match ShareNode::e2ee_received(&dir, share_id) {
            Ok(sp) => {
                if let Some(fk) = linked {
                    state
                        .with_conn(|c| import_shared_project(c, &sp))
                        .map(|_| ())?;
                    // Deletions propagate only from a CLEAN sync: undecryptable
                    // commits (no_key/failed) can hydrate the mirror incomplete,
                    // and prior − partial would delete content the host kept.
                    if outcome.no_key + outcome.failed == 0 {
                        if let Some(prior) = prior {
                            propagate_removals(state, share_id, &prior, &sp, fk)?;
                        }
                        advance_applied(&e2ee_received_dir(&dir), share_id);
                    }
                }
            }
            Err(linxiv_share::ShareError::NotFound(_)) => pending = true,
            Err(e) => return Err(e.into()),
        }
        let linked = linked.is_some();
        touch(&e2ee_reader_doc);
        // One line per reader sync, on stdout, so a terminal-launched app shows
        // what a stuck share is actually doing.
        println!(
            "share sync {share_id}: reader applied={} no_key={} failed={} mirror={} linked={linked}",
            outcome.applied,
            outcome.no_key,
            outcome.failed,
            if pending { "empty" } else { "populated" },
        );
        let undecryptable = outcome.no_key + outcome.failed;
        // The more specific key diagnosis wins over the generic pending line.
        // A re-keyed share leaves its pre-grant commits behind forever, so
        // undecryptable alone is not a fault — only diagnose one when nothing
        // came through. Nothing at all, and the usual cause is content sealed
        // BEFORE this device's invite (keyhive #136): those commits belong to
        // an epoch it never joined, and only a host re-key helps.
        let reason = pending.then(|| {
            if outcome.no_key > 0 {
                SyncReason::NoKeyForAnyContent
            } else if undecryptable > 0 {
                SyncReason::RevokedOrAwaitingKey
            } else {
                SyncReason::AwaitingFirstSync
            }
        });
        return to_value(&SyncedReceipt {
            e2ee: Some(true),
            applied: Some(outcome.applied),
            no_key: Some(outcome.no_key),
            failed: Some(outcome.failed),
            pending: pending.then_some(true),
            reason,
            undecryptable: (undecryptable > 0).then_some(undecryptable),
            ..synced(SyncRole::Reader)
        });
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

/// One best-effort sync pass over every share, sequential, log-and-continue.
/// Loop body of the interval sync; the headless bin runs its own loop over this.
pub async fn sync_all(state: &AppState, share: &ShareState) {
    let dir = share.share_dir().to_path_buf();
    // Dedupe ids; sync_share errors on a same-id doc in both dirs.
    let mut ids: std::collections::BTreeSet<String> = doc_ids(&dir).into_iter().collect();
    ids.extend(doc_ids(&received_dir(&dir)));
    ids.extend(doc_ids(&e2ee_dir(&dir)));
    ids.extend(doc_ids(&e2ee_received_dir(&dir)));
    for id in ids {
        if let Err(e) = sync_share(state, share, &id).await {
            eprintln!("share interval sync {id}: {} {}", e.status, e.detail);
        }
    }
}

/// Interval between best-effort passes of the background sync loop; each
/// front door (Tauri app, headless bin) spawns its own loop over [`sync_all`].
pub const INTERVAL_SYNC_PERIOD: Duration = Duration::from_secs(300);

/// Debounce after a mutation nudge before the pass runs, so a burst (bulk add)
/// coalesces into ~one sync instead of N.
pub const NUDGE_DEBOUNCE: Duration = Duration::from_secs(3);

static NUDGE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Poke the background sync loop after a write that may have touched shared
/// content. Cheap and non-blocking; `route()` calls this on successful non-GETs.
pub fn nudge() {
    NUDGE.notify_one();
    // The journal loop waits on its own Notify (notify_one wakes ONE waiter,
    // so two loops can't share this one).
    crate::journal::nudge();
}

/// Sleep until the next sync pass is due: the fixed interval, or sooner when a
/// mutation [`nudge`] arrives (plus [`NUDGE_DEBOUNCE`] so bursts coalesce).
pub async fn next_sync_due() {
    next_sync_due_on(&NUDGE, INTERVAL_SYNC_PERIOD, NUDGE_DEBOUNCE).await
}

pub(crate) async fn next_sync_due_on(
    nudge: &tokio::sync::Notify,
    interval: Duration,
    debounce: Duration,
) {
    tokio::select! {
        _ = tokio::time::sleep(interval) => {}
        _ = nudge.notified() => {
            tokio::time::sleep(debounce).await;
            // Drain nudges that landed during the debounce window: the pass
            // about to run covers them, and a leftover permit would re-fire
            // it immediately. Zero timeout polls `notified` once, consuming a
            // stored permit without blocking.
            let _ = tokio::time::timeout(Duration::ZERO, nudge.notified()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
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

    // Virtual time (start_paused): a nudge wakes the loop after the debounce
    // window, a mid-debounce nudge coalesces into the same pass (its permit is
    // drained), and an un-nudged wait runs the full interval.
    #[tokio::test(start_paused = true)]
    async fn nudge_wakes_early_and_burst_coalesces() {
        let n = std::sync::Arc::new(tokio::sync::Notify::new());
        let interval = Duration::from_secs(300);
        let debounce = Duration::from_secs(3);

        n.notify_one();
        let burst = n.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await; // lands mid-debounce
            burst.notify_one();
        });
        let t0 = tokio::time::Instant::now();
        next_sync_due_on(&n, interval, debounce).await;
        assert_eq!(t0.elapsed(), debounce);

        let t1 = tokio::time::Instant::now();
        next_sync_due_on(&n, interval, debounce).await;
        assert_eq!(t1.elapsed(), interval, "mid-debounce nudge must be drained");
    }

    // Value equality above is order-insensitive; the wire promise is byte-
    // identical key order to the old inline json! envelopes, so pin it here.
    #[test]
    fn envelope_key_order_matches_legacy_json() {
        let full = SyncedReceipt {
            e2ee: Some(true),
            applied: Some(1),
            no_key: Some(2),
            failed: Some(0),
            pending: Some(true),
            reason: Some(SyncReason::NoKeyForAnyContent),
            undecryptable: Some(2),
            ..synced(SyncRole::Reader)
        };
        assert_eq!(
            serde_json::to_string(&full).unwrap(),
            r#"{"synced":true,"role":"reader","e2ee":true,"applied":1,"no_key":2,"failed":0,"pending":true,"reason":"no key for any content","undecryptable":2}"#
        );
        let skip = SyncSkipped {
            synced: false,
            reason: SyncReason::Direction,
            role: Some(SyncRole::Hoster),
        };
        assert_eq!(
            serde_json::to_string(&skip).unwrap(),
            r#"{"synced":false,"reason":"direction","role":"hoster"}"#
        );
    }

    #[test]
    fn settings_default_unpaused_two_way() {
        let tmp = TempDir::new().unwrap();
        let settings = load_settings(tmp.path(), "nonexistent");
        assert!(!settings.paused);
        assert_eq!(settings.direction, SyncDirection::TwoWay);
    }

    // ── W4: e2ee legs ────────────────────────────────────────────────────────

    const E2EE_SID: &str = "44444444-4444-4444-8444-444444444444";

    fn e2ee_sample() -> linxiv_share::SharedProject {
        linxiv_share::SharedProject {
            share_id: E2EE_SID.into(),
            name: "E2ee P".into(),
            description: "d".into(),
            color: None,
            tags: vec!["v1".into()],
            papers: vec![],
            notes: vec![],
            annotations: vec![],
        }
    }

    // Keyhive/BeeKEM ops are slow in debug builds; generous per-op budget.
    async fn slow<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(60), fut)
            .await
            .expect("e2ee op should not hang on loopback")
    }

    #[tokio::test]
    async fn paused_e2ee_share_reports_paused() {
        let tmp = TempDir::new().unwrap();
        let state = mem_state();
        let share = ShareState::new(tmp.path());
        let doc = doc_path(&e2ee_received_dir(tmp.path()), E2EE_SID);
        fs::create_dir_all(doc.parent().unwrap()).unwrap();
        fs::write(&doc, "dummy").unwrap();
        let settings = ShareSettings {
            paused: true,
            ..Default::default()
        };
        save_settings(tmp.path(), E2EE_SID, &settings).unwrap();

        let v = sync_share(&state, &share, E2EE_SID).await.unwrap();
        assert_eq!(v, json!({ "synced": false, "reason": "paused" }));
    }

    #[tokio::test]
    async fn e2ee_project_gone_keeps_doc_and_settings_files() {
        let tmp = TempDir::new().unwrap();
        let state = mem_state();
        let share = ShareState::new(tmp.path());
        let doc = doc_path(&e2ee_dir(tmp.path()), E2EE_SID);
        fs::create_dir_all(doc.parent().unwrap()).unwrap();
        fs::write(&doc, "dummy").unwrap();
        save_settings(tmp.path(), E2EE_SID, &ShareSettings::default()).unwrap();

        let v = sync_share(&state, &share, E2EE_SID).await.unwrap();
        assert_eq!(v, json!({ "synced": false, "reason": "project gone" }));
        assert!(doc.exists());
        assert!(settings_path(tmp.path(), E2EE_SID).exists());
    }

    // Real bind pair over loopback: A's hoster leg rebuilds + republishes from
    // canonical, B's reader leg dials, refreshes the mirror, and imports into
    // the linked project.
    #[tokio::test(flavor = "multi_thread")]
    async fn e2ee_hoster_and_reader_legs_sync_over_loopback() {
        let a_dir = TempDir::new().unwrap();
        let b_dir = TempDir::new().unwrap();
        let node_a = ShareNode::bind_offline(a_dir.path(), &a_dir.path().join("p2p"))
            .await
            .unwrap();
        let node_b = ShareNode::bind_offline(b_dir.path(), &b_dir.path().join("p2p"))
            .await
            .unwrap();
        let share_a = ShareState::with_node(a_dir.path(), node_a);
        let share_b = ShareState::with_node(b_dir.path(), node_b);
        let state_a = mem_state();
        let state_b = mem_state();

        // A: canonical project linked to the share id + first secure publish.
        let sp = e2ee_sample();
        state_a
            .with_conn(|c| import_shared_project(c, &sp))
            .unwrap();
        let node_a = share_a.node().await.unwrap();
        slow(node_a.publish_secure(&sp)).await.unwrap();

        let v = slow(sync_share(&state_a, &share_a, E2EE_SID))
            .await
            .unwrap();
        // members=0: B is invited below, so nothing is granted yet.
        assert_eq!(
            v,
            json!({ "synced": true, "role": "hoster", "e2ee": true, "members": 0 })
        );

        // Invite B and link its mirror to a local project.
        let node_b = share_b.node().await.unwrap();
        let code = node_b.member_code().await.unwrap();
        let (_member, invite) = node_a
            .invite_member(E2EE_SID, &code, linxiv_share::Role::Read)
            .await
            .unwrap();
        assert_eq!(
            slow(node_b.accept_invite(&invite)).await.unwrap().share_id,
            E2EE_SID
        );
        let mirror = ShareNode::e2ee_received(b_dir.path(), E2EE_SID).unwrap();
        state_b
            .with_conn(|c| import_shared_project(c, &mirror))
            .unwrap();

        // A evolves the share; B's reader leg pulls and imports the change.
        let mut evolved = sp.clone();
        evolved.tags.push("post-invite".into());
        slow(node_a.publish_secure(&evolved)).await.unwrap();

        let v = slow(sync_share(&state_b, &share_b, E2EE_SID))
            .await
            .unwrap();
        assert_eq!(v["synced"], json!(true));
        assert_eq!(v["role"], json!("reader"));
        assert_eq!(v["e2ee"], json!(true));
        let fk = state_b
            .with_conn(|c| project_svc::find_by_share_id(c, E2EE_SID))
            .unwrap()
            .expect("mirror linked to a project");
        let p = state_b
            .with_conn(|c| {
                project_svc::get(
                    c,
                    &project_svc::Project {
                        project_fk: Some(fk),
                    },
                )
            })
            .unwrap()
            .unwrap();
        assert!(p.project_tags.contains(&"post-invite".to_string()));

        // A removes the tag again; B's next sync must propagate the deletion
        // (prior mirror − fresh mirror) instead of keeping it forever.
        slow(node_a.publish_secure(&sp)).await.unwrap();
        let v = slow(sync_share(&state_b, &share_b, E2EE_SID))
            .await
            .unwrap();
        assert_eq!(v["synced"], json!(true));
        let p = state_b
            .with_conn(|c| {
                project_svc::get(
                    c,
                    &project_svc::Project {
                        project_fk: Some(fk),
                    },
                )
            })
            .unwrap()
            .unwrap();
        assert!(
            !p.project_tags.contains(&"post-invite".to_string()),
            "remote tag removal must propagate to the linked project"
        );

        // Unlink B's local project, then sync again: the reader leg must keep
        // refreshing the mirror WITHOUT re-creating the project link — that
        // gate (find_by_share_id before import) is what the unlink feature's
        // "stops mirroring, membership survives" promise rests on.
        assert!(state_b
            .with_conn(|c| project_svc::release_share_id(c, E2EE_SID))
            .unwrap());
        let v = slow(sync_share(&state_b, &share_b, E2EE_SID))
            .await
            .unwrap();
        assert_eq!(v["synced"], json!(true));
        assert_eq!(v["role"], json!("reader"));
        assert_eq!(
            state_b
                .with_conn(|c| project_svc::find_by_share_id(c, E2EE_SID))
                .unwrap(),
            None,
            "sync must not re-link an unlinked share"
        );

        share_a.shutdown().await.unwrap();
        share_b.shutdown().await.unwrap();
    }

    // The hoster leg stores a blob ticket for a paper whose PDF is on disk, and
    // a later pass carries the ticket forward instead of wiping it on rebuild.
    #[tokio::test(flavor = "multi_thread")]
    async fn e2ee_hoster_leg_populates_pdf_blob_and_carries_it_forward() {
        let tmp = TempDir::new().unwrap();
        let pdf_dir = TempDir::new().unwrap();
        let mut conn = storage::open_in_memory().unwrap();
        storage::init_db(&conn).unwrap();
        let mut sp = e2ee_sample();
        sp.papers.push(linxiv_share::SharedPaper {
            source_id: "arxiv:9".into(),
            version: 1,
            published: None,
            title: "P".into(),
            summary: "s".into(),
            authors: vec![],
            tags: vec![],
            pdf_blob: None,
            author_orcids: vec![],
        });
        import_shared_project(&mut conn, &sp).unwrap();
        let state = AppState::from_parts(conn, pdf_dir.path().to_path_buf(), std::env::temp_dir());
        std::fs::write(
            pdf_dir
                .path()
                .join(linxiv_core::service::paper::pdf_on_disk_name("arxiv:9", 1)),
            b"%PDF-1.7 x",
        )
        .unwrap();

        let node = ShareNode::bind_offline(tmp.path(), &tmp.path().join("p2p"))
            .await
            .unwrap();
        let share = ShareState::with_node(tmp.path(), node);
        let node = share.node().await.unwrap();
        slow(node.publish_secure(&sp)).await.unwrap();

        let v = slow(sync_share(&state, &share, E2EE_SID)).await.unwrap();
        assert_eq!(v["synced"], json!(true));
        let doc = load(&e2ee_dir(tmp.path()), E2EE_SID).unwrap();
        let ticket = doc.papers[0].pdf_blob.clone().expect("blob ticket stored");

        slow(sync_share(&state, &share, E2EE_SID)).await.unwrap();
        let doc = load(&e2ee_dir(tmp.path()), E2EE_SID).unwrap();
        assert_eq!(doc.papers[0].pdf_blob, Some(ticket));

        share.shutdown().await.unwrap();
    }
}

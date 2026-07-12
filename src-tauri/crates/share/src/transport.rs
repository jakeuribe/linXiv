//! Share transport: a thin wrapper over the vendored `linxiv-p2p` sync node.
//!
//! A [`ShareNode`] owns one iroh endpoint with a persisted device key and serves
//! every locally-published doc (`share_dir/<id>.automerge`, top level only) over
//! the p2p sync ALPN. Received mirrors live under `share_dir/received/`; the
//! access check only allows ids whose doc file exists at the top level of
//! `share_dir`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use automerge::Automerge;
use linxiv_p2p::sync::JoinError;
use linxiv_p2p::DeviceIdentity;

pub use linxiv_p2p::{ShareTicket, ALPN};

use crate::{load, save, ShareError, SharedProject};
pub use linxiv_core::service::export_import::valid_share_id;

const RECEIVED_SUBDIR: &str = "received";
const DEVICE_KEY_FILE: &str = "device.key";

type Result<T> = std::result::Result<T, ShareError>;

fn net<E: std::fmt::Display>(e: E) -> ShareError {
    ShareError::Transport(format!("share transport: {e}"))
}

/// The directory received mirrors are materialized under.
pub fn received_dir(share_dir: &Path) -> PathBuf {
    share_dir.join(RECEIVED_SUBDIR)
}

/// Owns the p2p node for the share ALPN. One node both serves its own published
/// docs (via [`ShareNode::ticket`]) and fetches others' (via [`ShareNode::fetch`]).
pub struct ShareNode {
    inner: linxiv_p2p::ShareNode,
    share_dir: PathBuf,
}

impl ShareNode {
    /// Production node: iroh n0 defaults (relay + discovery). `share_dir` is the
    /// only directory served; `p2p_dir` holds the persisted device key.
    pub async fn bind(share_dir: impl Into<PathBuf>, p2p_dir: &Path) -> Result<Self> {
        Self::bind_inner(share_dir.into(), p2p_dir, false).await
    }

    /// Offline/hermetic node: no relays or discovery, direct addrs only. Used by
    /// tests and any same-host transfer.
    pub async fn bind_offline(share_dir: impl Into<PathBuf>, p2p_dir: &Path) -> Result<Self> {
        Self::bind_inner(share_dir.into(), p2p_dir, true).await
    }

    async fn bind_inner(share_dir: PathBuf, p2p_dir: &Path, offline: bool) -> Result<Self> {
        std::fs::create_dir_all(p2p_dir)?;
        let identity = DeviceIdentity::load_or_generate(p2p_dir.join(DEVICE_KEY_FILE))?;
        let inner = if offline {
            linxiv_p2p::ShareNode::bind_local(&identity).await
        } else {
            linxiv_p2p::ShareNode::bind(&identity).await
        }
        .map_err(net)?;
        let node = Self { inner, share_dir };
        // Serve only what is published at the top level of share_dir; received/
        // mirrors land in the p2p registry after a join.
        let allowed_dir = node.share_dir.clone();
        node.inner.set_access_check(Arc::new(move |_peer, id| {
            valid_share_id(id) && super::doc_path(&allowed_dir, id).is_file()
        }));
        node.register_published().await?;
        Ok(node)
    }

    /// Register every published doc (top-level `*.automerge` only). A corrupt
    /// doc is skipped, mirroring `list_shared`.
    async fn register_published(&self) -> Result<()> {
        let entries = match std::fs::read_dir(&self.share_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("automerge") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let _ = self.refresh(id).await;
        }
        Ok(())
    }

    /// (Re-)register `share_id` from its doc file, so the p2p registry serves the
    /// latest published bytes. Missing file → `NotFound`. Reads and decodes the
    /// doc off the async runtime via `spawn_blocking`.
    pub async fn refresh(&self, share_id: &str) -> Result<()> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        let doc_path = super::doc_path(&self.share_dir, share_id);
        let id = share_id.to_string();
        let doc = tokio::task::spawn_blocking(move || -> Result<Automerge> {
            let bytes = match std::fs::read(&doc_path) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(ShareError::NotFound(id))
                }
                Err(e) => return Err(e.into()),
            };
            Automerge::load(&bytes).map_err(super::crdt)
        })
        .await
        .map_err(net)??;
        self.inner.register(share_id, doc);
        Ok(())
    }

    /// Build a pasteable ticket for a published share. Refreshes the registered
    /// doc from disk first; an unpublished id errors.
    pub async fn ticket(&self, share_id: &str) -> Result<ShareTicket> {
        self.refresh(share_id).await?;
        self.inner.ticket(share_id).map_err(net)
    }

    /// Dial the ticket's host, sync the doc, and materialize a READ-ONLY mirror
    /// under `dest_share_dir/received`.
    pub async fn fetch(
        &self,
        ticket: &ShareTicket,
        dest_share_dir: &Path,
    ) -> Result<SharedProject> {
        let share_id = ticket.project_id();
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(
                "share not found or revoked".to_string(),
            ));
        }
        self.inner.join(ticket).await.map_err(|e| match e {
            JoinError::Refused => ShareError::NotFound("share not found or revoked".to_string()),
            JoinError::Other(e) => net(e),
        })?;
        let doc = self
            .inner
            .doc(share_id)
            .ok_or_else(|| net("synced doc missing from registry"))?;
        let sp: SharedProject = autosurgeon::hydrate(&doc).map_err(super::crdt)?;
        // The doc-internal share_id is attacker-controlled and feeds save()'s paths.
        if !valid_share_id(&sp.share_id) {
            return Err(net(format!("remote share_id is unsafe: {:?}", sp.share_id)));
        }
        if sp.share_id != share_id {
            return Err(net(format!(
                "remote share_id {:?} does not match ticket id {share_id:?}",
                sp.share_id
            )));
        }
        // Namespaced mirror dir.
        save(&received_dir(dest_share_dir), &sp)?;
        Ok(sp)
    }

    /// Hydrate a received mirror by id from `dest_share_dir/received`.
    pub fn received(dest_share_dir: &Path, share_id: &str) -> Result<SharedProject> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        load(&received_dir(dest_share_dir), share_id)
    }

    /// Summaries of every received mirror under `dest_share_dir/received`.
    pub fn list_received(dest_share_dir: &Path) -> Result<Vec<crate::SharedSummary>> {
        crate::list_shared(&received_dir(dest_share_dir))
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.inner.shutdown().await.map_err(net)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample(share_id: &str, name: &str) -> SharedProject {
        SharedProject {
            share_id: share_id.into(),
            name: name.into(),
            description: "desc".into(),
            color: Some(0x00ff00),
            tags: vec!["RL".into(), "Robotics".into()],
            papers: vec![crate::SharedPaper {
                source_id: "arxiv:1".into(),
                version: 1,
                title: "First".into(),
                summary: "s".into(),
                authors: vec!["Alice".into()],
                tags: vec!["ml".into()],
                published: None,
            }],
            notes: vec![crate::SharedNote {
                uuid: "11111111-1111-4111-8111-111111111111".into(),
                paper_source_id: Some("arxiv:1".into()),
                title: "n".into(),
                body: "b".into(),
                created_at: None,
                updated_at: None,
            }],
            annotations: vec![crate::SharedAnnotation {
                uuid: "22222222-2222-4222-8222-222222222222".into(),
                paper_source_id: "arxiv:1".into(),
                anchor: "{}".into(),
                comment: "c".into(),
                created_at: None,
                updated_at: None,
            }],
        }
    }

    async fn node(dir: &Path) -> ShareNode {
        ShareNode::bind_offline(dir, &dir.join("p2p"))
            .await
            .unwrap()
    }

    async fn with_timeout<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("p2p op should not hang on loopback")
    }

    // Real endpoint<->endpoint QUIC over loopback: A publishes + tickets, B
    // fetches into received/ and the mirror hydrates + lists.
    #[tokio::test(flavor = "multi_thread")]
    async fn loopback_fetch_matches_sender() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = node(a_dir.path()).await;
        let b = node(b_dir.path()).await;

        let sp = sample("7", "My Project");
        save(a_dir.path(), &sp).unwrap();
        let ticket = with_timeout(a.ticket("7")).await.unwrap();
        // Ticket round-trips through its pasteable string form.
        let ticket: ShareTicket = ticket.to_string().parse().unwrap();

        let got = with_timeout(b.fetch(&ticket, b_dir.path())).await.unwrap();
        assert_eq!(got, sp);
        assert!(b_dir.path().join("received").join("7.automerge").is_file());
        assert_eq!(ShareNode::received(b_dir.path(), "7").unwrap(), sp);
        let listed = ShareNode::list_received(b_dir.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].share_id, "7");

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    // A ticket for a share the host no longer publishes must surface as NotFound
    // on the joining side, and never clobber a same-id local publish.
    #[tokio::test(flavor = "multi_thread")]
    async fn revoked_share_join_is_not_found() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = node(a_dir.path()).await;
        let b = node(b_dir.path()).await;

        save(a_dir.path(), &sample("7", "Sender Project")).unwrap();
        let local = sample("7", "My Local Project");
        save(b_dir.path(), &local).unwrap();

        let ticket = with_timeout(a.ticket("7")).await.unwrap();
        // Unpublish: the access check now refuses id "7".
        std::fs::remove_file(a_dir.path().join("7.automerge")).unwrap();

        let result = with_timeout(b.fetch(&ticket, b_dir.path())).await;
        assert!(
            matches!(result, Err(ShareError::NotFound(_))),
            "refused share must surface as NotFound, got {result:?}"
        );
        assert_eq!(crate::load(b_dir.path(), "7").unwrap(), local);

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    // Quarantine: B's received mirror of A's share must not be re-servable — a
    // third node fetching from B with A's share id gets refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn received_mirror_is_not_reserved() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let c_dir = tempfile::tempdir().unwrap();
        let a = node(a_dir.path()).await;
        let b = node(b_dir.path()).await;
        let c = node(c_dir.path()).await;

        save(a_dir.path(), &sample("7", "A's Project")).unwrap();
        let ticket = with_timeout(a.ticket("7")).await.unwrap();
        with_timeout(b.fetch(&ticket, b_dir.path())).await.unwrap();
        // B now holds "7" in its p2p registry and as a received/ mirror.

        // B publishes its own "8" so we can learn B's dialable address.
        save(b_dir.path(), &sample("8", "B's Project")).unwrap();
        let b_ticket = with_timeout(b.ticket("8")).await.unwrap();
        let evil = ShareTicket::new(b_ticket.endpoint_addr().clone(), "7");

        let result = with_timeout(c.fetch(&evil, c_dir.path())).await;
        assert!(
            matches!(result, Err(ShareError::NotFound(_))),
            "a received mirror must not be re-served, got {result:?}"
        );
        assert!(!c_dir.path().join("received").join("7.automerge").exists());

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        c.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ticket_for_unpublished_id_errors() {
        let dir = tempfile::tempdir().unwrap();
        let n = node(dir.path()).await;
        let result = with_timeout(n.ticket("nope")).await;
        assert!(
            matches!(result, Err(ShareError::NotFound(_))),
            "unpublished id must not mint a ticket, got {result:?}"
        );
        n.shutdown().await.unwrap();
    }

    // A malicious host serves a doc whose INTERNAL share_id is a traversal path.
    // fetch() must reject it before save() and write nothing outside received/.
    #[tokio::test(flavor = "multi_thread")]
    async fn malicious_share_id_is_rejected_no_write() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = node(a_dir.path()).await;
        let b = node(b_dir.path()).await;

        // Doc filename is the safe "7"; the share_id FIELD inside is a traversal.
        let mut evil = sample("7", "Evil");
        evil.share_id = "../evil".into();
        let mut doc = automerge::AutoCommit::new();
        autosurgeon::reconcile(&mut doc, &evil).unwrap();
        std::fs::write(a_dir.path().join("7.automerge"), doc.save()).unwrap();

        let ticket = with_timeout(a.ticket("7")).await.unwrap();
        let result = with_timeout(b.fetch(&ticket, b_dir.path())).await;
        assert!(
            matches!(result, Err(ShareError::Transport(_))),
            "malicious share_id must be rejected, got {result:?}"
        );
        assert!(!b_dir.path().join("evil.automerge").exists());
        assert!(!b_dir
            .path()
            .join("received")
            .join("evil.automerge")
            .exists());

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
}

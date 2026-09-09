//! Share transport over the vendored `linxiv-p2p` node: a [`ShareNode`] serves only
//! top-level `share_dir/<id>.automerge` docs (the access check requires the file to
//! exist there); received mirrors are quarantined under `share_dir/received/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use automerge::Automerge;
use linxiv_p2p::sync::JoinError;
use linxiv_p2p::DeviceIdentity;

#[cfg(feature = "sync-beelay")]
pub use linxiv_p2p::{MemberId, ProjectInvite, Role};
pub use linxiv_p2p::{ShareTicket, ALPN};

use crate::{load, save, ShareError, SharedProject};
pub use linxiv_core::service::export_import::valid_share_id;

const RECEIVED_SUBDIR: &str = "received";
const DEVICE_KEY_FILE: &str = "device.key";
#[cfg(feature = "sync-beelay")]
const E2EE_SUBDIR: &str = "e2ee";

type Result<T> = std::result::Result<T, ShareError>;

fn net<E: std::fmt::Display>(e: E) -> ShareError {
    ShareError::Transport(format!("share transport: {e}"))
}

/// The directory received mirrors are materialized under.
pub fn received_dir(share_dir: &Path) -> PathBuf {
    share_dir.join(RECEIVED_SUBDIR)
}

/// Hoster-published e2ee docs: `share_dir/e2ee/<id>.automerge`.
#[cfg(feature = "sync-beelay")]
pub fn e2ee_dir(share_dir: &Path) -> PathBuf {
    share_dir.join(E2EE_SUBDIR)
}

/// Reader mirrors of e2ee shares: `share_dir/e2ee/received/<id>.automerge`.
#[cfg(feature = "sync-beelay")]
pub fn e2ee_received_dir(share_dir: &Path) -> PathBuf {
    e2ee_dir(share_dir).join(RECEIVED_SUBDIR)
}

/// What one [`ShareNode::accept_invite`] achieved.
#[cfg(feature = "sync-beelay")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedInvite {
    pub share_id: String,
    /// The host was unreachable, so the join is half done: the invite is parked,
    /// the mirror on disk is an empty placeholder, and the interval sync finishes it.
    pub pending: bool,
}

/// What one [`ShareNode::sync_e2ee`] changed locally.
#[cfg(feature = "sync-beelay")]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct E2eeSyncOutcome {
    /// Decrypted changes newly applied to the local doc.
    pub applied: usize,
    /// Commits with no key for their epoch: revoked, or not yet keyed.
    pub no_key: usize,
    /// Commits that failed to decrypt for any other reason.
    pub failed: usize,
}

#[cfg(feature = "sync-beelay")]
impl From<linxiv_p2p::SyncOutcome> for E2eeSyncOutcome {
    fn from(o: linxiv_p2p::SyncOutcome) -> Self {
        use linxiv_p2p::DecryptError;
        let no_key = o
            .undecryptable
            .iter()
            .filter(|e| matches!(e, DecryptError::KeyNotFound))
            .count();
        Self {
            applied: o.applied,
            no_key,
            failed: o.undecryptable.len() - no_key,
        }
    }
}

/// Raw doc bytes to `dir/<share_id>.automerge` via tmp+rename (mirror writes
/// preserve remote CRDT history byte-for-byte, unlike `save`'s reconcile).
fn write_doc_bytes(dir: &Path, share_id: &str, bytes: Vec<u8>) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{share_id}.automerge.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, super::doc_path(dir, share_id))?;
    Ok(())
}

#[cfg(feature = "sync-beelay")]
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Lowercase hex of a member id — the form member ids take on the wire and in
/// the members sidecar.
#[cfg(feature = "sync-beelay")]
pub fn member_id_hex(m: &MemberId) -> String {
    m.0.iter().map(|b| format!("{b:02x}")).collect()
}

/// Inverse of [`member_id_hex`]; `None` unless `s` is exactly 32 hex bytes.
#[cfg(feature = "sync-beelay")]
pub fn member_id_from_hex(s: &str) -> Option<MemberId> {
    Some(MemberId(decode_hex(s)?.try_into().ok()?))
}

/// Owns the p2p node for the share ALPN. One node both serves its own published
/// docs (via [`ShareNode::ticket`]) and fetches others' (via [`ShareNode::fetch`]).
pub struct ShareNode {
    inner: linxiv_p2p::ShareNode,
    share_dir: PathBuf,
    // None when the keyhive auth state failed to load.
    #[cfg(feature = "sync-beelay")]
    beelay: Option<linxiv_p2p::BeelayNode>,
}

impl ShareNode {
    /// Production node: iroh n0 defaults (relay + discovery). `share_dir` is the
    /// only directory served; `p2p_dir` holds the persisted device key.
    pub async fn bind(share_dir: impl Into<PathBuf>, p2p_dir: &Path) -> Result<Self> {
        Self::bind_inner(share_dir.into(), p2p_dir, false, None, None).await
    }

    /// Like [`Self::bind`]; `Some(dek)` AEAD-wraps the at-rest key files under the
    /// 32-byte DEK (legacy plaintext migrates once), `None` keeps plaintext.
    /// `relay` swaps in a self-hosted relay; `None` keeps n0 defaults.
    #[cfg(feature = "sync-beelay")]
    pub async fn bind_with_dek(
        share_dir: impl Into<PathBuf>,
        p2p_dir: &Path,
        dek: Option<[u8; 32]>,
        relay: Option<linxiv_p2p::CustomRelay>,
    ) -> Result<Self> {
        Self::bind_inner(share_dir.into(), p2p_dir, false, relay, dek).await
    }

    /// Offline/hermetic node: no relays or discovery, direct addrs only. Used by
    /// tests and any same-host transfer.
    pub async fn bind_offline(share_dir: impl Into<PathBuf>, p2p_dir: &Path) -> Result<Self> {
        Self::bind_inner(share_dir.into(), p2p_dir, true, None, None).await
    }

    // `_dek` is only read with the beelay stack; the underscore keeps the
    // plain-sync build warning-free.
    async fn bind_inner(
        share_dir: PathBuf,
        p2p_dir: &Path,
        offline: bool,
        relay: Option<linxiv_p2p::CustomRelay>,
        _dek: Option<[u8; 32]>,
    ) -> Result<Self> {
        std::fs::create_dir_all(p2p_dir)?;
        #[cfg(feature = "sync-beelay")]
        let identity = DeviceIdentity::load_or_generate_with_dek(
            p2p_dir.join(DEVICE_KEY_FILE),
            _dek.as_ref(),
        )?;
        #[cfg(not(feature = "sync-beelay"))]
        let identity = DeviceIdentity::load_or_generate(p2p_dir.join(DEVICE_KEY_FILE))?;
        #[cfg(feature = "sync-beelay")]
        let (inner, beelay) =
            Self::bind_stack(&identity, p2p_dir, offline, relay, _dek.as_ref()).await?;
        #[cfg(not(feature = "sync-beelay"))]
        let inner = Self::bind_plain(&identity, offline, relay).await?;
        let node = Self {
            inner,
            share_dir,
            #[cfg(feature = "sync-beelay")]
            beelay,
        };
        // Serve only what is published at the top level of share_dir; received/
        // mirrors land in the p2p registry after a join.
        let allowed_dir = node.share_dir.clone();
        node.inner.set_access_check(Arc::new(move |_peer, id| {
            valid_share_id(id) && super::doc_path(&allowed_dir, id).is_file()
        }));
        node.register_published().await?;
        Ok(node)
    }

    async fn bind_plain(
        identity: &DeviceIdentity,
        offline: bool,
        relay: Option<linxiv_p2p::CustomRelay>,
    ) -> Result<linxiv_p2p::ShareNode> {
        if offline {
            linxiv_p2p::ShareNode::bind_local(identity).await
        } else if let Some(relay) = relay {
            linxiv_p2p::ShareNode::bind_custom_relay(identity, relay).await
        } else {
            linxiv_p2p::ShareNode::bind(identity).await
        }
        .map_err(net)
    }

    /// Plain sync + beelay + blobs on ONE endpoint. Corrupt keyhive state (or an
    /// auth key that won't decrypt) falls back to a plain bind with no beelay node.
    #[cfg(feature = "sync-beelay")]
    async fn bind_stack(
        identity: &DeviceIdentity,
        p2p_dir: &Path,
        offline: bool,
        relay: Option<linxiv_p2p::CustomRelay>,
        dek: Option<&[u8; 32]>,
    ) -> Result<(linxiv_p2p::ShareNode, Option<linxiv_p2p::BeelayNode>)> {
        let auth_identity = match linxiv_p2p::AuthIdentity::load_or_generate_with_dek(
            p2p_dir.join("auth.key"),
            dek,
        ) {
            Ok(identity) => identity,
            Err(e) => {
                tracing::warn!("keyhive auth key failed to load, e2ee sharing disabled: {e}");
                return Ok((Self::bind_plain(identity, offline, relay).await?, None));
            }
        };
        let auth = match linxiv_p2p::ProjectAuth::load_or_new_with_dek(
            &auth_identity,
            &p2p_dir.join("keyhive"),
            dek,
        )
        .await
        {
            Ok(auth) => auth,
            Err(e) => {
                tracing::warn!("keyhive auth state failed to load, e2ee sharing disabled: {e}");
                return Ok((Self::bind_plain(identity, offline, relay).await?, None));
            }
        };
        let beelay_dir = p2p_dir.join("beelay");
        let stack = if offline {
            linxiv_p2p::bind_stack_local(identity, &auth_identity, auth, Some(&beelay_dir)).await
        } else if let Some(relay) = relay.clone() {
            linxiv_p2p::bind_stack_custom_relay(
                identity,
                &auth_identity,
                auth,
                Some(&beelay_dir),
                relay,
            )
            .await
        } else {
            linxiv_p2p::bind_stack(identity, &auth_identity, auth, Some(&beelay_dir)).await
        };
        match stack {
            Ok((inner, beelay)) => Ok((inner, Some(beelay))),
            Err(e) => {
                tracing::warn!("beelay/blob stack failed to bind, e2ee sharing disabled: {e}");
                Ok((Self::bind_plain(identity, offline, relay).await?, None))
            }
        }
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

    /// (Re-)register `share_id` from its doc file so the registry serves the latest
    /// bytes; missing file → `NotFound`. Reads/decodes off the runtime via `spawn_blocking`.
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

    /// Register `share_id` from the just-reconciled doc `save()` returned, skipping
    /// [`Self::refresh`]'s re-read. Callers must have `save()`d the doc first, or the
    /// access check will refuse to serve it.
    pub fn register_doc(&self, share_id: &str, mut doc: automerge::AutoCommit) -> Result<()> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        self.inner.register(share_id, doc.document().clone());
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
        // Namespaced mirror dir — raw bytes, so the host's actors/timestamps
        // survive into the mirror's history instead of being re-authored as
        // this device by save()'s reconcile.
        write_doc_bytes(&received_dir(dest_share_dir), share_id, doc.save())?;
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

    /// This node's iroh endpoint id (the device's share identity), as hex.
    pub fn endpoint_id(&self) -> String {
        self.inner.endpoint_id().to_string()
    }

    /// The underlying iroh endpoint — Remote Query Mode's client half dials
    /// remote nodes from it (one endpoint, never a second bind).
    pub fn endpoint(&self) -> &linxiv_p2p::Endpoint {
        self.inner.endpoint()
    }

    /// Remote Query Mode: installs the `linxiv-api/1` handler on this endpoint.
    /// Never called by the desktop app — until installed, api-ALPN connections are refused.
    pub fn set_api_protocol(&self, handler: Box<dyn linxiv_p2p::DynProtocolHandler>) {
        self.inner.set_api_protocol(handler);
    }

    pub async fn shutdown(&self) -> Result<()> {
        // Shared router under bind_stack.
        #[cfg(feature = "sync-beelay")]
        let beelay_res = match &self.beelay {
            Some(beelay) => beelay.shutdown().await.map_err(net),
            None => Ok(()),
        };
        let inner_res = self.inner.shutdown().await.map_err(net);
        #[cfg(feature = "sync-beelay")]
        beelay_res?;
        inner_res
    }
}

/// E2EE sharing over the beelay + keyhive stack. Docs live under
/// `share_dir/e2ee` (hoster) and `share_dir/e2ee/received` (reader mirrors).
#[cfg(feature = "sync-beelay")]
impl ShareNode {
    fn beelay(&self) -> Result<&linxiv_p2p::BeelayNode> {
        self.beelay.as_ref().ok_or_else(|| {
            ShareError::Transport(
                "e2ee sharing unavailable: keyhive auth state failed to load".to_string(),
            )
        })
    }

    /// Publish (or republish) a project as an e2ee share: evolve the doc under
    /// `share_dir/e2ee`, then register/merge it in beelay. Content encrypts at invite/sync time.
    pub async fn publish_secure(&self, sp: &SharedProject) -> Result<()> {
        let beelay = self.beelay()?;
        if !valid_share_id(&sp.share_id) {
            return Err(ShareError::NotFound(sp.share_id.clone()));
        }
        let dir = e2ee_dir(&self.share_dir);
        let sp_owned = sp.clone();
        let mut doc = tokio::task::spawn_blocking(move || -> Result<Automerge> {
            save(&dir, &sp_owned)?;
            let bytes = std::fs::read(super::doc_path(&dir, &sp_owned.share_id))?;
            Automerge::load(&bytes).map_err(super::crdt)
        })
        .await
        .map_err(net)??;
        if beelay.auth().doc_id(&sp.share_id).is_some() {
            beelay
                .with_doc(&sp.share_id, |d| d.merge(&mut doc).map(|_| ()))
                .await
                .ok_or_else(|| net("e2ee project missing from beelay registry"))?
                .map_err(super::crdt)?;
        } else {
            beelay
                .create_shared_project(&sp.share_id, doc)
                .await
                .map_err(net)?;
        }
        Ok(())
    }

    /// This device's own keyhive member id — how the hoster itself appears in
    /// a members listing.
    pub fn self_member_id(&self) -> Result<MemberId> {
        Ok(self.beelay()?.auth().member_id())
    }

    /// This device's pasteable membership code (its keyhive contact card,
    /// hex). Give it to a hoster so they can [`ShareNode::invite_member`] you.
    pub async fn member_code(&self) -> Result<String> {
        let card = self.beelay()?.auth().contact_card().await.map_err(net)?;
        Ok(card.iter().map(|b| format!("{b:02x}")).collect())
    }

    /// Grant the device behind `member_code` `role` on an e2ee share and mint its
    /// pasteable invite. Returns the member id (the revoke/query_role handle) + invite string.
    pub async fn invite_member(
        &self,
        share_id: &str,
        member_code: &str,
        role: Role,
    ) -> Result<(MemberId, String)> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        let beelay = self.beelay()?;
        let card = decode_hex(member_code).ok_or_else(|| net("malformed member code"))?;
        let member = beelay
            .auth()
            .receive_contact_card(&card)
            .await
            .map_err(net)?;
        match beelay
            .auth()
            .query_access(share_id, member)
            .await
            .map_err(net)?
        {
            // Same role already granted: just re-mint the invite string.
            Some(existing) if existing == role => {
                return beelay
                    .invite(share_id, member)
                    .await
                    .map(|invite| (member, invite))
                    .map_err(net);
            }
            Some(_) => {
                return Err(ShareError::RoleConflict);
            }
            None => {}
        }
        beelay
            .auth()
            .add_member(share_id, member, role)
            .await
            .map_err(net)?;
        match beelay.invite(share_id, member).await {
            Ok(invite) => Ok((member, invite)),
            Err(e) => {
                if let Err(re) = beelay.auth().revoke_member(share_id, member).await {
                    let hex = member_id_hex(&member);
                    tracing::warn!(
                        "compensating revoke after failed invite also failed for member {hex}: {re}"
                    );
                    return Err(net(format!(
                        "invite failed: {e}; compensating revoke of member {hex} also failed: {re}; revoke it manually"
                    )));
                }
                Err(net(e))
            }
        }
    }

    /// The member's effective role on an e2ee share; `None` = no access.
    pub async fn query_role(&self, share_id: &str, member: MemberId) -> Result<Option<Role>> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        self.beelay()?
            .auth()
            .query_access(share_id, member)
            .await
            .map_err(net)
    }

    /// Change a member's role (viewer ↔ editor). A downgrade rotates the project key
    /// (PCS) — re-key stored blobs afterwards. Dropping the doc's last reader
    /// surfaces as [`ShareError::LastReader`].
    pub async fn set_role(&self, share_id: &str, member: MemberId, role: Role) -> Result<()> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        self.beelay()?
            .auth()
            .set_role(share_id, member, role)
            .await
            .map_err(|e| match e.downcast_ref::<linxiv_p2p::SetRoleError>() {
                Some(linxiv_p2p::SetRoleError::LastReader) => ShareError::LastReader,
                None => net(e),
            })
    }

    /// Revoke a member; the project key rotates (PCS).
    pub async fn revoke(&self, share_id: &str, member: MemberId) -> Result<()> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        self.beelay()?
            .auth()
            .revoke_member(share_id, member)
            .await
            .map_err(net)
    }

    /// Encrypt `bytes` under the share key and serve them as a blob; returns a
    /// pasteable ticket. Size caps are the caller's job — the blobs API exposes no size before fetch.
    pub async fn store_pdf_blob(&self, share_id: &str, bytes: &[u8]) -> Result<String> {
        self.beelay()?
            .store_blob(share_id, bytes)
            .await
            .map_err(net)
    }

    /// Read + decrypt a blob, fetching it from its host first when not local.
    /// `max_bytes` bounds the transferred/decrypted size (caller's quota).
    pub async fn read_pdf_blob(
        &self,
        share_id: &str,
        ticket: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        use linxiv_p2p::beelay::BlobError;
        let beelay = self.beelay()?;
        let classify = |e: linxiv_p2p::AnyError| match e.downcast_ref::<BlobError>() {
            Some(BlobError::Decrypt { .. }) => {
                ShareError::NotFound("access revoked or removed".to_string())
            }
            Some(be @ BlobError::TooLarge) => ShareError::TooLarge(be.to_string()),
            _ => net(e),
        };
        if beelay.has_blob(ticket).await {
            return beelay
                .read_blob(share_id, ticket, max_bytes)
                .await
                .map_err(classify);
        }
        beelay
            .fetch_blob(ticket, max_bytes)
            .await
            .map_err(classify)?;
        beelay
            .read_blob(share_id, ticket, max_bytes)
            .await
            .map_err(classify)
    }

    /// Accept an e2ee invite: adopt the share, sync once, and mirror it under
    /// `share_dir/e2ee/received`. An unreachable host is not an error.
    pub async fn accept_invite(&self, invite: &str) -> Result<AcceptedInvite> {
        let beelay = self.beelay()?;
        // The invite's project id feeds file paths below; reject unsafe ids
        // before adopting anything.
        let parsed: linxiv_p2p::ProjectInvite = invite.parse().map_err(net)?;
        if !valid_share_id(parsed.project_id()) {
            return Err(net(format!(
                "invite share_id is unsafe: {:?}",
                parsed.project_id()
            )));
        }
        let share_id = beelay.accept_invite(invite).await.map_err(net)?;
        // Persist the mirror before the first sync so the interval loop
        // retries this share when that sync fails.
        if let Some(doc) = beelay.doc(&share_id).await {
            // Validate the doc-internal share_id (host-controlled) BEFORE the
            // mirror lands on disk: a prior failed sync can leave a hostile
            // merge in the in-memory doc, and this write must never persist
            // it. A fresh adopt is an empty doc — nothing to validate.
            if !doc.get_heads().is_empty() {
                let doc_id = doc_share_id(&doc)?;
                if doc_id != share_id {
                    return Err(net(format!(
                        "remote share_id {doc_id:?} does not match invite id {share_id:?}"
                    )));
                }
            }
            let dir = e2ee_received_dir(&self.share_dir);
            let id = share_id.clone();
            tokio::task::spawn_blocking(move || write_doc_bytes(&dir, &id, doc.save()))
                .await
                .map_err(net)??;
        }
        // The host never answered: the adoption is parked, so there is nothing
        // to fetch and no point failing on a sync against the same dead host.
        // The placeholder mirror written above is what the interval loop finds.
        if beelay.join_pending(&share_id) {
            return Ok(AcceptedInvite {
                share_id,
                pending: true,
            });
        }
        self.sync_e2ee(&share_id).await?;
        Ok(AcceptedInvite {
            share_id,
            pending: false,
        })
    }

    /// Sync a received e2ee mirror and persist it under `e2ee/received/`. A hosted
    /// share_id errors (hosted docs update via [`ShareNode::publish_secure`] only);
    /// a host refusal (revoked/removed) surfaces as `NotFound`.
    pub async fn sync_e2ee(&self, share_id: &str) -> Result<E2eeSyncOutcome> {
        let beelay = self.beelay()?;
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        if super::doc_path(&e2ee_dir(&self.share_dir), share_id).is_file() {
            return Err(net(format!(
                "sync_e2ee on hosted share {share_id}: hosted docs update via publish_secure"
            )));
        }
        let outcome: E2eeSyncOutcome = beelay
            .sync_project(share_id)
            .await
            .map_err(|e| match e {
                JoinError::Refused => ShareError::NotFound("access revoked or removed".to_string()),
                JoinError::Other(e) => net(e),
            })?
            .into();
        let doc = beelay
            .doc(share_id)
            .await
            .ok_or_else(|| net("synced doc missing from beelay registry"))?;
        // Nothing decrypted into the doc yet — the host was asleep at join time,
        // or every commit came back no-key (revoked / not yet keyed). There is
        // nothing to hydrate or mirror; the outcome carries the counts, and a
        // later pass fills it in. Same empty-doc guard as accept_invite.
        if doc.get_heads().is_empty() {
            return Ok(outcome);
        }
        // The doc-internal share_id is host-controlled; same check as fetch().
        let doc_id = doc_share_id(&doc)?;
        if doc_id != share_id {
            return Err(net(format!(
                "remote share_id {doc_id:?} does not match invite id {share_id:?}"
            )));
        }
        let dir = e2ee_received_dir(&self.share_dir);
        let id = share_id.to_string();
        tokio::task::spawn_blocking(move || write_doc_bytes(&dir, &id, doc.save()))
            .await
            .map_err(net)??;
        Ok(outcome)
    }

    /// Re-encrypt a hosted e2ee share's whole history under the current epoch —
    /// repairs shares invited before invites did this themselves.
    /// TODO: Revisit if this should be exposed via the GUI
    pub async fn rekey_e2ee(&self, share_id: &str) -> Result<()> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        if !super::doc_path(&e2ee_dir(&self.share_dir), share_id).is_file() {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        self.beelay()?.reseal_project(share_id).await.map_err(net)
    }

    /// Undo a join: drop the beelay registration, cached doc, and any parked invite
    /// so a re-accept adopts from scratch. Returns whether beelay had it registered;
    /// the caller deletes the on-disk mirror.
    pub async fn forget_e2ee(&self, share_id: &str) -> Result<bool> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        if super::doc_path(&e2ee_dir(&self.share_dir), share_id).is_file() {
            return Err(net(format!(
                "forget_e2ee on hosted share {share_id}: unpublish it instead"
            )));
        }
        self.beelay()?.forget_project(share_id).await.map_err(net)
    }

    /// Summaries of locally-published e2ee shares (`share_dir/e2ee`).
    pub fn list_e2ee(share_dir: &Path) -> Result<Vec<crate::SharedSummary>> {
        crate::list_shared(&e2ee_dir(share_dir))
    }

    /// Summaries of received e2ee mirrors (`share_dir/e2ee/received`).
    pub fn list_e2ee_received(share_dir: &Path) -> Result<Vec<crate::SharedSummary>> {
        crate::list_shared(&e2ee_received_dir(share_dir))
    }

    /// Hydrate a received e2ee mirror by id.
    pub fn e2ee_received(share_dir: &Path, share_id: &str) -> Result<SharedProject> {
        if !valid_share_id(share_id) {
            return Err(ShareError::NotFound(share_id.to_string()));
        }
        load(&e2ee_received_dir(share_dir), share_id)
    }
}

/// The doc-internal `share_id`, hydrated alone — the host-controlled-id guard's
/// one input, so the check never materializes the full SharedProject subgraphs.
#[cfg(feature = "sync-beelay")]
fn doc_share_id(doc: &Automerge) -> Result<String> {
    #[derive(autosurgeon::Hydrate)]
    struct Meta {
        share_id: String,
    }
    let meta: Meta = autosurgeon::hydrate(doc).map_err(super::crdt)?;
    Ok(meta.share_id)
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
                author_orcids: vec![],
                tags: vec!["ml".into()],
                published: None,
                pdf_blob: None,
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

    // The sync tick's save + register_doc path (no refresh re-read) must serve
    // the updated doc: B's second fetch with the original ticket sees the edit.
    #[tokio::test(flavor = "multi_thread")]
    async fn register_doc_serves_the_updated_doc() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = node(a_dir.path()).await;
        let b = node(b_dir.path()).await;

        save(a_dir.path(), &sample("7", "Before")).unwrap();
        let ticket = with_timeout(a.ticket("7")).await.unwrap();
        with_timeout(b.fetch(&ticket, b_dir.path())).await.unwrap();

        let after = sample("7", "After");
        let doc = save(a_dir.path(), &after).unwrap();
        a.register_doc("7", doc).unwrap();

        let got = with_timeout(b.fetch(&ticket, b_dir.path())).await.unwrap();
        assert_eq!(got, after);

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

    #[cfg(feature = "sync-beelay")]
    mod e2ee {
        use super::*;

        // keyhive/BeeKEM ops are slow in debug builds; give e2ee flows more
        // room than the 10s plain-loopback budget.
        async fn slow<T>(fut: impl std::future::Future<Output = T>) -> T {
            tokio::time::timeout(Duration::from_secs(60), fut)
                .await
                .expect("e2ee op should not hang on loopback")
        }

        // publish_secure -> member_code -> invite_member -> accept_invite on
        // a second node, returning the reader's member id on the hoster.
        async fn share_to(a: &ShareNode, b: &ShareNode, sp: &SharedProject) -> MemberId {
            a.publish_secure(sp).await.unwrap();
            let code = b.member_code().await.unwrap();
            let (member, invite) = a
                .invite_member(&sp.share_id, &code, Role::Read)
                .await
                .unwrap();
            let accepted = slow(b.accept_invite(&invite)).await.unwrap();
            assert_eq!(accepted.share_id, sp.share_id);
            // both nodes are live here: the join must complete, not park
            assert!(!accepted.pending, "loopback accept should not be pending");
            member
        }

        // Pasting an invite whose host is asleep is a success, not an error:
        // the invite is parked and a placeholder mirror lands so the interval
        // loop retries. The share stays out of the received listing until that
        // first sync fills it in (list_shared skips a mirror whose hydrated id
        // does not match its filename).
        #[tokio::test(flavor = "multi_thread")]
        async fn offline_invite_accept_is_pending_not_an_error() {
            let a_dir = tempfile::tempdir().unwrap();
            let b_dir = tempfile::tempdir().unwrap();
            let a = node(a_dir.path()).await;
            let b = node(b_dir.path()).await;

            let sp = sample("7", "Asleep Host");
            a.publish_secure(&sp).await.unwrap();
            let code = b.member_code().await.unwrap();
            let (_member, invite) = a.invite_member("7", &code, Role::Read).await.unwrap();
            a.shutdown().await.unwrap();

            let accepted = slow(b.accept_invite(&invite)).await.unwrap();
            assert_eq!(accepted.share_id, "7");
            assert!(accepted.pending, "an unreachable host must report pending");

            // placeholder mirror exists so the sync loop can find it again...
            assert!(b_dir.path().join("e2ee/received/7.automerge").is_file());
            // ...but it carries no content yet, so it is not listed
            assert!(
                ShareNode::list_e2ee_received(b_dir.path())
                    .unwrap()
                    .is_empty(),
                "a pending share should not surface as a joined one"
            );

            b.shutdown().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn publish_invite_accept_materializes_reader_mirror() {
            let a_dir = tempfile::tempdir().unwrap();
            let b_dir = tempfile::tempdir().unwrap();
            let a = node(a_dir.path()).await;
            let b = node(b_dir.path()).await;

            let sp = sample("7", "Secret Project");
            let member = share_to(&a, &b, &sp).await;

            // reader mirror lives under e2ee/received and matches the content
            assert!(b_dir.path().join("e2ee/received/7.automerge").is_file());
            assert_eq!(ShareNode::e2ee_received(b_dir.path(), "7").unwrap(), sp);
            let listed = ShareNode::list_e2ee_received(b_dir.path()).unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0].share_id, "7");
            assert_eq!(ShareNode::list_e2ee(a_dir.path()).unwrap().len(), 1);
            assert_eq!(a.query_role("7", member).await.unwrap(), Some(Role::Read));

            // plain top-level dirs untouched on both sides
            assert!(!a_dir.path().join("7.automerge").exists());
            assert!(!b_dir.path().join("7.automerge").exists());
            assert!(!b_dir.path().join("received").exists());

            a.shutdown().await.unwrap();
            b.shutdown().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn revoked_reader_sync_errors_and_mirror_is_intact() {
            let a_dir = tempfile::tempdir().unwrap();
            let b_dir = tempfile::tempdir().unwrap();
            let a = node(a_dir.path()).await;
            let b = node(b_dir.path()).await;

            let sp = sample("7", "Secret Project");
            let member = share_to(&a, &b, &sp).await;

            a.revoke("7", member).await.unwrap();
            assert_eq!(a.query_role("7", member).await.unwrap(), None);
            // hoster evolves the share after the revoke
            let mut evolved = sp.clone();
            evolved.tags.push("post-revoke".into());
            a.publish_secure(&evolved).await.unwrap();

            let result = slow(b.sync_e2ee("7")).await;
            assert!(
                matches!(&result, Err(ShareError::NotFound(m)) if m.contains("revoked")),
                "revoked sync must surface as a revocation, got {result:?}"
            );
            // old mirror content intact, no post-revoke content gained
            assert_eq!(ShareNode::e2ee_received(b_dir.path(), "7").unwrap(), sp);

            a.shutdown().await.unwrap();
            b.shutdown().await.unwrap();
        }

        // §3.3: set_role transitions Read→Edit→Read; query_role is the truth.
        #[tokio::test(flavor = "multi_thread")]
        async fn set_role_upgrades_and_downgrades() {
            let a_dir = tempfile::tempdir().unwrap();
            let b_dir = tempfile::tempdir().unwrap();
            let a = node(a_dir.path()).await;
            let b = node(b_dir.path()).await;

            let sp = sample("7", "Secret Project");
            let member = share_to(&a, &b, &sp).await;
            assert_eq!(a.query_role("7", member).await.unwrap(), Some(Role::Read));

            slow(a.set_role("7", member, Role::Edit)).await.unwrap();
            assert_eq!(a.query_role("7", member).await.unwrap(), Some(Role::Edit));

            slow(a.set_role("7", member, Role::Read)).await.unwrap();
            assert_eq!(a.query_role("7", member).await.unwrap(), Some(Role::Read));

            a.shutdown().await.unwrap();
            b.shutdown().await.unwrap();
        }

        // §6 validate-before-persist: a host serving a doc whose INTERNAL
        // share_id mismatches the invite id must never land in the reader's
        // mirror — neither via the first sync (validated in sync_e2ee) nor
        // via a re-accept persisting the poisoned in-memory doc.
        #[tokio::test(flavor = "multi_thread")]
        async fn hostile_share_id_never_persisted_on_accept() {
            let a_dir = tempfile::tempdir().unwrap();
            let b_dir = tempfile::tempdir().unwrap();
            let a = node(a_dir.path()).await;
            let b = node(b_dir.path()).await;

            let sp = sample("7", "Evil Host");
            a.publish_secure(&sp).await.unwrap();
            // Swap the doc-internal share_id before the invite (invite
            // flushes/encrypts pending changes, so the reader will fetch it).
            let mut evil = sp.clone();
            evil.share_id = "8".into();
            a.beelay
                .as_ref()
                .unwrap()
                .with_doc("7", |d| {
                    let mut tx = d.transaction();
                    autosurgeon::reconcile(&mut tx, &evil).unwrap();
                    tx.commit();
                })
                .await
                .unwrap();
            let code = b.member_code().await.unwrap();
            let (_member, invite) = a.invite_member("7", &code, Role::Read).await.unwrap();

            // First accept: the initial sync pulls the hostile doc → error.
            let r1 = slow(b.accept_invite(&invite)).await;
            assert!(
                matches!(&r1, Err(ShareError::Transport(m)) if m.contains("does not match")),
                "hostile doc must error the accept, got {r1:?}"
            );
            // Re-accept: the hostile merge now sits in the in-memory doc;
            // the pre-persist validation must refuse to park it.
            let r2 = slow(b.accept_invite(&invite)).await;
            assert!(
                matches!(&r2, Err(ShareError::Transport(m)) if m.contains("does not match")),
                "re-accept must not persist the hostile doc, got {r2:?}"
            );
            // Whatever is on disk (empty placeholder or nothing), it is
            // never the hostile doc.
            match ShareNode::e2ee_received(b_dir.path(), "7") {
                Err(_) => {}
                Ok(mirror) => assert_ne!(mirror.share_id, "8", "hostile doc landed in the mirror"),
            }

            a.shutdown().await.unwrap();
            b.shutdown().await.unwrap();
        }

        // Quarantine: an e2ee doc is invisible to the plain sync path — a
        // plain ticket forged for its id gets refused and writes nothing.
        #[tokio::test(flavor = "multi_thread")]
        async fn e2ee_doc_is_never_served_by_plain_path() {
            let a_dir = tempfile::tempdir().unwrap();
            let b_dir = tempfile::tempdir().unwrap();
            let a = node(a_dir.path()).await;
            let b = node(b_dir.path()).await;

            a.publish_secure(&sample("7", "Secret")).await.unwrap();
            // publish a plain "8" only to learn a's dialable address
            save(a_dir.path(), &sample("8", "Public")).unwrap();
            let ticket = with_timeout(a.ticket("8")).await.unwrap();
            let evil = ShareTicket::new(ticket.endpoint_addr().clone(), "7");

            let result = with_timeout(b.fetch(&evil, b_dir.path())).await;
            assert!(
                matches!(result, Err(ShareError::NotFound(_))),
                "an e2ee doc must not be plain-fetchable, got {result:?}"
            );
            assert!(!b_dir.path().join("received").join("7.automerge").exists());

            a.shutdown().await.unwrap();
            b.shutdown().await.unwrap();
        }
    }
}

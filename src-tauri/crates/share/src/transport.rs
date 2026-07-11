//! Phase-1 one-way share transport over iroh.
//!
//! A [`ShareNode`] owns an iroh `Endpoint` + `Router` serving a dedicated ALPN.
//! The Phase-0 `share_id` is the guessable project id, so it is NOT the network
//! capability: minting a ticket generates an unguessable per-share `CapToken`
//! and the server serves a doc only when a presented token resolves to a known
//! `share_id`. The handler reads ONLY from the share directory — it never holds
//! a handle to `papers.db`.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, Watcher};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::{load, save, ShareError, SharedProject};

/// Dedicated ALPN for the one-way public-share protocol.
pub const ALPN: &[u8] = b"linxiv/public-share/0";

const CAP_BYTES: usize = 16;
const CAPS_SUBDIR: &str = "caps";
const RECEIVED_SUBDIR: &str = "received";
// Bounds for the finish-delimited bi-stream reads.
const MAX_TOKEN_LEN: usize = 64;
const MAX_DOC_LEN: usize = 64 * 1024 * 1024;
// Server-side per-connection deadline so a stalled peer can't pin a handler task
// until the QUIC idle timeout; the closed() tail gets a shorter grace.
const PER_CONN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

type Result<T> = std::result::Result<T, ShareError>;

fn net<E: std::fmt::Display>(e: E) -> ShareError {
    ShareError::Transport(format!("share transport: {e}"))
}

// ── capability tokens ───────────────────────────────────────────────────────

/// An unguessable per-share capability (>=16 random bytes). The encoded form is
/// base32 (no padding), safe as both a filename and a ticket field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapToken(Vec<u8>);

impl CapToken {
    fn generate() -> Self {
        let mut buf = vec![0u8; CAP_BYTES];
        rand::thread_rng().fill_bytes(&mut buf);
        CapToken(buf)
    }

    pub fn encode(&self) -> String {
        data_encoding::BASE32_NOPAD.encode(&self.0)
    }

    pub fn decode(s: &str) -> Result<Self> {
        data_encoding::BASE32_NOPAD
            .decode(s.as_bytes())
            .map(CapToken)
            .map_err(net)
    }
}

fn caps_dir(share_dir: &Path) -> PathBuf {
    share_dir.join(CAPS_SUBDIR)
}

// True unless `id` could escape share_dir when joined as a path segment.
fn valid_share_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.contains(['/', '\\'])
        && !id.contains("..")
        && !Path::new(id).is_absolute()
}

/// Mint an unguessable token for `share_id` and persist `token -> share_id` as a
/// file under `share_dir/caps`. Re-minting first revokes this share's prior
/// tokens, so `caps/` holds one live token per share and old tickets stop working.
pub async fn mint_capability(share_dir: &Path, share_id: &str) -> Result<CapToken> {
    let dir = caps_dir(share_dir);
    tokio::fs::create_dir_all(&dir).await?;
    revoke_capabilities(share_dir, share_id).await?;
    let cap = CapToken::generate();
    tokio::fs::write(dir.join(cap.encode()), share_id.as_bytes()).await?;
    Ok(cap)
}

/// Delete every cap file whose contents name `share_id`. Scans `caps/` (token ->
/// share_id files); minting is rare, so the O(n) sweep is cheap.
pub async fn revoke_capabilities(share_dir: &Path, share_id: &str) -> Result<()> {
    let dir = caps_dir(share_dir);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        match tokio::fs::read(&path).await {
            Ok(bytes) if bytes == share_id.as_bytes() => tokio::fs::remove_file(&path).await?,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Resolve a presented token to its `share_id`, or `None` if unknown. The token
/// is the file name, so an unknown token simply finds no file (no enumeration).
pub async fn resolve_capability(share_dir: &Path, cap: &CapToken) -> Result<Option<String>> {
    // A token shorter than a minted one can never match; an empty token would also
    // encode to "" and read the caps dir itself (IsADirectory, not NotFound).
    if cap.0.len() < CAP_BYTES {
        return Ok(None);
    }
    match tokio::fs::read(caps_dir(share_dir).join(cap.encode())).await {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes).map_err(net)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// ── pasteable ticket ────────────────────────────────────────────────────────

/// Everything a receiver needs to dial one share: the sender's `EndpointAddr`
/// (node id + direct addrs) and the capability that unlocks its doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShareTicket {
    pub addr: EndpointAddr,
    pub cap: CapToken,
}

impl std::fmt::Display for ShareTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bytes = postcard::to_stdvec(self).map_err(|_| std::fmt::Error)?;
        f.write_str(&data_encoding::BASE32_NOPAD.encode(&bytes))
    }
}

impl FromStr for ShareTicket {
    type Err = ShareError;
    fn from_str(s: &str) -> Result<Self> {
        let bytes = data_encoding::BASE32_NOPAD
            .decode(s.as_bytes())
            .map_err(net)?;
        postcard::from_bytes(&bytes).map_err(net)
    }
}

// ── server-side protocol handler ────────────────────────────────────────────

/// Serves CRDT doc bytes for a presented capability, reading ONLY from
/// `share_dir`. Unknown tokens are refused without touching anything else.
#[derive(Debug, Clone)]
struct ShareProtocol {
    share_dir: PathBuf,
}

impl ProtocolHandler for ShareProtocol {
    async fn accept(
        &self,
        connection: Connection,
    ) -> std::result::Result<(), iroh::protocol::AcceptError> {
        tokio::time::timeout(PER_CONN_TIMEOUT, self.serve(&connection))
            .await
            .map_err(AcceptError::from_err)?
    }
}

impl ShareProtocol {
    async fn serve(&self, connection: &Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let token_bytes = recv
            .read_to_end(MAX_TOKEN_LEN)
            .await
            .map_err(AcceptError::from_err)?;
        let cap = CapToken(token_bytes);

        let share_id = resolve_capability(&self.share_dir, &cap)
            .await
            .map_err(AcceptError::from_err)?;
        // Unknown token, or a cap file naming a path outside share_dir: close the
        // stream empty so the client reads zero bytes instead of a doc.
        let serveable = share_id.filter(|id| valid_share_id(id));
        let Some(share_id) = serveable else {
            send.finish().map_err(AcceptError::from_err)?;
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, connection.closed()).await;
            return Ok(());
        };

        let doc = tokio::fs::read(super::doc_path(&self.share_dir, &share_id))
            .await
            .map_err(AcceptError::from_err)?;
        send.write_all(&doc).await.map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        let _ = tokio::time::timeout(CLOSE_TIMEOUT, connection.closed()).await;
        Ok(())
    }
}

// ── the node ────────────────────────────────────────────────────────────────

/// Owns the iroh `Endpoint` + `Router` for the public-share ALPN. One node both
/// serves its own shares (via [`ShareNode::ticket`]) and fetches others'.
pub struct ShareNode {
    endpoint: Endpoint,
    router: Router,
    share_dir: PathBuf,
    mint_lock: tokio::sync::Mutex<()>,
}

impl ShareNode {
    /// Production node: iroh n0 defaults (relay + discovery). `share_dir` is the
    /// only directory the inbound handler may read.
    pub async fn bind(share_dir: impl Into<PathBuf>) -> Result<Self> {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(net)?;
        Self::with_endpoint(endpoint, share_dir.into()).await
    }

    /// Offline/hermetic node: relay disabled, no address lookup, direct addrs
    /// only. Used by tests and any same-host transfer.
    pub async fn bind_offline(share_dir: impl Into<PathBuf>) -> Result<Self> {
        // `Minimal` sets only the mandatory ring crypto provider; we add relay-off
        // and no address lookup so the node is fully hermetic (direct addrs only).
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .clear_address_lookup()
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .map_err(net)?;
        Self::with_endpoint(endpoint, share_dir.into()).await
    }

    async fn with_endpoint(endpoint: Endpoint, share_dir: PathBuf) -> Result<Self> {
        let router = Router::builder(endpoint.clone())
            .accept(
                ALPN,
                ShareProtocol {
                    share_dir: share_dir.clone(),
                },
            )
            .spawn();
        Ok(Self {
            endpoint,
            router,
            share_dir,
            mint_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// This node's dialable address once at least one direct addr is known.
    async fn addr(&self) -> Result<EndpointAddr> {
        tokio::time::timeout(PER_CONN_TIMEOUT, async {
            let mut watcher = self.endpoint.watch_addr();
            loop {
                let addr = watcher.get();
                if addr.ip_addrs().next().is_some() || addr.relay_urls().next().is_some() {
                    return Ok(addr);
                }
                watcher.updated().await.map_err(net)?;
            }
        })
        .await
        .map_err(net)?
    }

    /// Mint a capability for `share_id` and build a pasteable ticket from this
    /// node's address. Caller must have published `share_id` into `share_dir`.
    pub async fn ticket(&self, share_id: &str) -> Result<ShareTicket> {
        let cap = {
            // Serialize the mint's scan-delete-write across concurrent callers.
            let _g = self.mint_lock.lock().await;
            mint_capability(&self.share_dir, share_id).await?
        };
        Ok(ShareTicket {
            addr: self.addr().await?,
            cap,
        })
    }

    /// Dial the ticket's sender, present the capability, receive the doc bytes,
    /// and materialize a READ-ONLY mirror under `dest_share_dir/received` so it
    /// can't clobber a locally-published project with the same `share_id`.
    pub async fn fetch(
        &self,
        ticket: &ShareTicket,
        dest_share_dir: &Path,
    ) -> Result<SharedProject> {
        let conn = self
            .endpoint
            .connect(ticket.addr.clone(), ALPN)
            .await
            .map_err(net)?;
        let (mut send, mut recv) = conn.open_bi().await.map_err(net)?;
        send.write_all(&ticket.cap.0).await.map_err(net)?;
        send.finish().map_err(net)?;

        let doc = recv.read_to_end(MAX_DOC_LEN).await.map_err(net)?;
        conn.close(0u32.into(), b"done");
        if doc.is_empty() {
            return Err(ShareError::NotFound("capability refused".into()));
        }

        let doc = automerge::AutoCommit::load(&doc).map_err(super::crdt)?;
        let sp: SharedProject = autosurgeon::hydrate(&doc).map_err(super::crdt)?;
        // The doc-internal share_id is attacker-controlled and feeds save()'s paths.
        if !valid_share_id(&sp.share_id) {
            return Err(net(format!("remote share_id is unsafe: {:?}", sp.share_id)));
        }
        // Namespaced mirror dir: never overwrites dest/<share_id>.automerge.
        let mirror = dest_share_dir.join(RECEIVED_SUBDIR);
        save(&mirror, &sp)?;
        Ok(sp)
    }

    /// Hydrate a received mirror by id from `dest_share_dir/received`.
    pub fn received(dest_share_dir: &Path, share_id: &str) -> Result<SharedProject> {
        load(&dest_share_dir.join(RECEIVED_SUBDIR), share_id)
    }

    /// Summaries of every received mirror under `dest_share_dir/received`.
    pub fn list_received(dest_share_dir: &Path) -> Result<Vec<crate::SharedSummary>> {
        crate::list_shared(&dest_share_dir.join(RECEIVED_SUBDIR))
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await.map_err(net)?;
        Ok(())
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
            }],
            notes: vec![crate::SharedNote {
                id: 1,
                title: "n".into(),
                body: "b".into(),
                created_at: None,
                updated_at: None,
            }],
            annotations: vec![crate::SharedAnnotation {
                id: 1,
                paper_source_id: "arxiv:1".into(),
                anchor: "{}".into(),
                comment: "c".into(),
                created_at: None,
                updated_at: None,
            }],
        }
    }

    #[test]
    fn ticket_encode_decode_roundtrip() {
        let secret = iroh::SecretKey::generate();
        let addr =
            EndpointAddr::new(secret.public()).with_ip_addr("127.0.0.1:4242".parse().unwrap());
        let ticket = ShareTicket {
            addr,
            cap: CapToken::generate(),
        };
        let encoded = ticket.to_string();
        let decoded: ShareTicket = encoded.parse().unwrap();
        assert_eq!(ticket, decoded);
    }

    #[tokio::test]
    async fn capability_mint_resolve_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cap = mint_capability(dir.path(), "42").await.unwrap();
        assert_eq!(
            resolve_capability(dir.path(), &cap)
                .await
                .unwrap()
                .as_deref(),
            Some("42")
        );

        // A freshly generated token that was never minted resolves to nothing.
        let unknown = CapToken::generate();
        assert!(resolve_capability(dir.path(), &unknown)
            .await
            .unwrap()
            .is_none());

        // A zero-byte token must resolve to nothing, not read the caps dir itself.
        assert!(resolve_capability(dir.path(), &CapToken(vec![]))
            .await
            .unwrap()
            .is_none());

        // Re-minting revokes the prior token: only the newest stays valid.
        let cap2 = mint_capability(dir.path(), "42").await.unwrap();
        assert!(resolve_capability(dir.path(), &cap)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            resolve_capability(dir.path(), &cap2)
                .await
                .unwrap()
                .as_deref(),
            Some("42")
        );
    }

    // Real Endpoint<->Endpoint QUIC over loopback: relays disabled, no discovery,
    // receiver dials the sender's direct localhost addr from the ticket.
    #[tokio::test(flavor = "multi_thread")]
    async fn loopback_fetch_matches_sender() {
        let sender_dir = tempfile::tempdir().unwrap();
        let recv_dir = tempfile::tempdir().unwrap();

        let sender = ShareNode::bind_offline(sender_dir.path()).await.unwrap();
        let receiver = ShareNode::bind_offline(recv_dir.path()).await.unwrap();

        let sp = sample("7", "My Project");
        save(sender_dir.path(), &sp).unwrap();
        let ticket = tokio::time::timeout(Duration::from_secs(10), sender.ticket("7"))
            .await
            .expect("addr should resolve on loopback")
            .unwrap();

        let got = tokio::time::timeout(
            Duration::from_secs(10),
            receiver.fetch(&ticket, recv_dir.path()),
        )
        .await
        .expect("fetch should not hang")
        .unwrap();

        assert_eq!(got, sp);
        // Materialized as a namespaced read-only mirror, not at the top level.
        assert_eq!(ShareNode::received(recv_dir.path(), "7").unwrap(), sp);

        sender.shutdown().await.unwrap();
        receiver.shutdown().await.unwrap();
    }

    // A SUCCESSFUL fetch into a dir already holding a locally-published <id> doc
    // must land in received/ and leave the top-level local doc untouched.
    #[tokio::test(flavor = "multi_thread")]
    async fn successful_fetch_does_not_clobber_local() {
        let sender_dir = tempfile::tempdir().unwrap();
        let recv_dir = tempfile::tempdir().unwrap();

        let sender = ShareNode::bind_offline(sender_dir.path()).await.unwrap();
        let receiver = ShareNode::bind_offline(recv_dir.path()).await.unwrap();

        let sender_sp = sample("7", "Sender Project");
        save(sender_dir.path(), &sender_sp).unwrap();

        // Receiver already locally published a DIFFERENT project under the same id.
        let local_sp = sample("7", "My Local Project");
        save(recv_dir.path(), &local_sp).unwrap();

        let ticket = tokio::time::timeout(Duration::from_secs(10), sender.ticket("7"))
            .await
            .unwrap()
            .unwrap();
        let got = tokio::time::timeout(
            Duration::from_secs(10),
            receiver.fetch(&ticket, recv_dir.path()),
        )
        .await
        .expect("fetch should not hang")
        .unwrap();

        assert_eq!(got, sender_sp);
        // The mirror holds the sender's doc; the top-level local doc is untouched.
        assert_eq!(
            ShareNode::received(recv_dir.path(), "7").unwrap(),
            sender_sp
        );
        assert_eq!(load(recv_dir.path(), "7").unwrap(), local_sp);

        sender.shutdown().await.unwrap();
        receiver.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_capability_is_refused_and_no_clobber() {
        let sender_dir = tempfile::tempdir().unwrap();
        let recv_dir = tempfile::tempdir().unwrap();

        let sender = ShareNode::bind_offline(sender_dir.path()).await.unwrap();
        let receiver = ShareNode::bind_offline(recv_dir.path()).await.unwrap();

        let sender_sp = sample("7", "Sender Project");
        save(sender_dir.path(), &sender_sp).unwrap();

        // Receiver already locally published a DIFFERENT project under the same id.
        let local_sp = sample("7", "My Local Project");
        save(recv_dir.path(), &local_sp).unwrap();

        // A real ticket address but an unminted capability: server must refuse.
        let mut ticket = tokio::time::timeout(Duration::from_secs(10), sender.ticket("7"))
            .await
            .unwrap()
            .unwrap();
        ticket.cap = CapToken::generate();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            receiver.fetch(&ticket, recv_dir.path()),
        )
        .await
        .expect("refusal should not hang");
        assert!(
            matches!(result, Err(ShareError::NotFound(_))),
            "unknown capability must surface as NotFound (404), got {result:?}"
        );

        // The receiver's locally-published project is untouched.
        assert_eq!(load(recv_dir.path(), "7").unwrap(), local_sp);

        sender.shutdown().await.unwrap();
        receiver.shutdown().await.unwrap();
    }

    // A malicious server serves a doc whose INTERNAL share_id is a traversal path.
    // fetch() must reject it before save() and write nothing outside received/.
    #[tokio::test(flavor = "multi_thread")]
    async fn malicious_share_id_is_rejected_no_write() {
        let sender_dir = tempfile::tempdir().unwrap();
        let recv_dir = tempfile::tempdir().unwrap();

        let sender = ShareNode::bind_offline(sender_dir.path()).await.unwrap();
        let receiver = ShareNode::bind_offline(recv_dir.path()).await.unwrap();

        // Doc filename is the safe "7"; the share_id FIELD inside is a traversal.
        let mut evil = sample("7", "Evil");
        evil.share_id = "../evil".into();
        let mut doc = automerge::AutoCommit::new();
        autosurgeon::reconcile(&mut doc, &evil).unwrap();
        std::fs::write(sender_dir.path().join("7.automerge"), doc.save()).unwrap();

        let ticket = tokio::time::timeout(Duration::from_secs(10), sender.ticket("7"))
            .await
            .unwrap()
            .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            receiver.fetch(&ticket, recv_dir.path()),
        )
        .await
        .expect("rejection should not hang");
        assert!(
            matches!(result, Err(ShareError::Transport(_))),
            "malicious share_id must be rejected, got {result:?}"
        );
        // Nothing escaped received/ to recv_dir/evil.automerge.
        assert!(!recv_dir.path().join("evil.automerge").exists());

        sender.shutdown().await.unwrap();
        receiver.shutdown().await.unwrap();
    }
}

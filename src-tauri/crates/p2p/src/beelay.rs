//! Beelay sync over iroh (Phase 3, feature `sync-beelay`): keyhive-encrypted
//! automerge changes travel as opaque beelay commits.
//!
//! Wire format on ALPN `linxiv/beelay/0`, reusing Phase 1's 8-byte LE length
//! frames; the first payload byte is a tag: 1 = keyhive static-events
//! preamble (exchanged + ingested in BOTH directions before any beelay
//! traffic — "CGKA ops first"), 0 = a beelay stream `Message`
//! (`Connecting`/`Connected` hello handshake, then the envelope pump).
//! A zero-length frame ends the session; the responder acks it with another
//! zero-length frame once it has applied everything it received.
//!
//! Each automerge change maps to exactly one beelay commit: contents =
//! [`ProjectAuth::encrypt`] of the change bytes, hash = blake3(ciphertext),
//! parents = the change's deps mapped through a change-hash -> commit-hash
//! map. The receive side rebuilds that map by decrypting commits back into
//! changes, so both sides agree on the DAG without ever shipping plaintext.
//! DAG shape (parents/hashes) travels unencrypted — a beelay design decision.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use automerge::{Automerge, Change, ChangeHash};
use beelay_core::{
    Beelay, Commit, CommitCategory, CommitHash, CommitOrBundle, DocumentId, Envelope, Event,
    PeerId, StorageKey, StoryId, StoryResult,
    io::{IoAction, IoResult, IoTask},
    messages::stream::{Connected, Connecting, Message, Step},
};
use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::{Connection, ConnectionError, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{
    BlobFormat, BlobsProtocol,
    api::{Store, remote::GetProgressItem},
    provider::{
        events::{AbortReason, EventMask, EventSender, ObserveMode, ProviderMessage, RequestMode},
        handle_connection,
    },
    store::fs::FsStore,
    store::mem::MemStore,
    ticket::BlobTicket,
};
use iroh_tickets::{ParseError, Ticket, endpoint::EndpointTicket};
use n0_error::{AnyError, Result, StackResultExt, StdResultExt, anyerr};
use rand::rngs::OsRng;
use tokio::sync::Mutex;

use crate::{
    auth::{AuthIdentity, DecryptError, DeviceBinding, MemberId, ProjectAuth, Role},
    sync::{
        DeviceIdentity, JoinError, MAX_SYNC_ROUNDS, RECV_TIMEOUT, REFUSED_CODE, ShareNode,
        recv_frame, recv_frame_max, send_frame,
    },
};

/// ALPN for linXiv beelay sync sessions.
pub const BEELAY_ALPN: &[u8] = b"linxiv/beelay/0";

/// Frame tag: beelay stream `Message` bytes.
const TAG_BEELAY: u8 = 0;
/// Frame tag: keyhive preamble payload (contact card or static events).
const TAG_KEYHIVE: u8 = 1;

// --- change <-> commit mapping ----------------------------------------------

/// Changes in `doc` that have no commit yet, ordered deps-before-dependents
/// (robust to whatever order `get_changes` returns).
fn unflushed_changes(
    doc: &Automerge,
    mapped: &HashMap<ChangeHash, CommitHash>,
) -> Result<Vec<Change>> {
    // ponytail: full-history scan per flush; fine at project scale, switch to
    // get_changes(&flushed_heads) bookkeeping if it ever shows in a profile.
    let mut pending: Vec<Change> = doc
        .get_changes(&[])
        .into_iter()
        .filter(|c| !mapped.contains_key(&c.hash()))
        .collect();
    let mut ready: HashSet<ChangeHash> = mapped.keys().copied().collect();
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let (take, keep): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|c| c.deps().iter().all(|d| ready.contains(d)));
        if take.is_empty() {
            // every doc change is either mapped or pending, so this is a bug
            return Err(anyerr!("change dependency cycle or unmapped dep"));
        }
        for change in take {
            ready.insert(change.hash());
            ordered.push(change);
        }
        pending = keep;
    }
    Ok(ordered)
}

/// The change's deps as beelay commit hashes.
fn commit_parents(
    change: &Change,
    mapped: &HashMap<ChangeHash, CommitHash>,
) -> Result<Vec<CommitHash>> {
    change
        .deps()
        .iter()
        .map(|dep| {
            mapped
                .get(dep)
                .copied()
                .ok_or_else(|| anyerr!("change dep {dep} has no commit mapping"))
        })
        .collect()
}

// --- sync outcome ------------------------------------------------------------

/// What one sync (or refresh) changed locally.
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Decrypted changes newly applied to the local document.
    pub applied: usize,
    /// Commits fetched but not decryptable — [`DecryptError::KeyNotFound`]
    /// means content from an epoch this device is not in (revoked, or
    /// pre-grant content: keyhive #136).
    pub undecryptable: Vec<DecryptError>,
}

// --- blob errors ---------------------------------------------------------------

/// Typed blob failure, wrapped as the direct [`AnyError`] payload so
/// callers can `downcast_ref::<BlobError>()` it.
#[derive(Debug)]
pub enum BlobError {
    /// The blob is larger than the caller's byte cap.
    TooLarge,
    /// The stored ciphertext did not decrypt.
    Decrypt(DecryptError),
}

impl fmt::Display for BlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlobError::TooLarge => f.write_str("blob exceeds the byte cap"),
            BlobError::Decrypt(e) => write!(f, "decrypting blob: {e}"),
        }
    }
}

impl std::error::Error for BlobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BlobError::TooLarge => None,
            BlobError::Decrypt(e) => Some(e),
        }
    }
}

// vendor-edit: typed [`BeelayNode::sync_project`] failure for a registry
// entry whose stored host ticket failed to parse at load, wrapped as the
// direct [`AnyError`] payload so callers can
// `downcast_ref::<HostTicketError>()` it (same pattern as `BlobError` /
// `SetRoleError`).
#[derive(Debug, PartialEq, Eq)]
pub enum HostTicketError {
    /// The registry's stored host ticket is unreadable; rejoin the share
    /// (accept a fresh invite) to repair the address.
    Unreadable,
}

impl fmt::Display for HostTicketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostTicketError::Unreadable => {
                f.write_str("stored host ticket is unreadable; rejoin the share")
            }
        }
    }
}

impl std::error::Error for HostTicketError {}

// --- invite ------------------------------------------------------------------

/// A pasteable invite for one member: host address, project/doc ids, and the
/// keyhive delegation events that member needs.
///
/// Create it with [`BeelayNode::invite`] AFTER granting the member via
/// [`ProjectAuth::add_member`] — content is encrypted at invite time so it
/// lands in an epoch the member can read (grant-before-encrypt, keyhive #136).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInvite {
    endpoint: EndpointTicket,
    project_id: String,
    beelay_doc: [u8; 16],
    keyhive_doc: [u8; 32],
    // vendor-edit: membership group rides the invite so the invitee can manage
    // membership (per its granted role) after adopting.
    group: [u8; 32],
    events: Vec<u8>,
}

impl ProjectInvite {
    /// The shared project's id.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

impl Ticket for ProjectInvite {
    const KIND: &'static str = "linxivinvite";

    fn encode_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            &self.endpoint,
            &self.project_id,
            self.beelay_doc,
            self.keyhive_doc,
            self.group,
            &self.events,
        ))
        .expect("postcard serialization failed")
    }

    fn decode_bytes(bytes: &[u8]) -> std::result::Result<Self, ParseError> {
        let (endpoint, project_id, beelay_doc, keyhive_doc, group, events) =
            postcard::from_bytes(bytes)?;
        Ok(Self {
            endpoint,
            project_id,
            beelay_doc,
            keyhive_doc,
            group,
            events,
        })
    }
}

impl fmt::Display for ProjectInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode_string())
    }
}

impl FromStr for ProjectInvite {
    type Err = ParseError;

    fn from_str(s: &str) -> std::result::Result<Self, ParseError> {
        Self::decode_string(s)
    }
}

// --- beelay core wrapper -------------------------------------------------------

// vendor-edit: disk-backed KV — postcard file helpers shared by the beelay
// storage map and the project registry; writes are tmp+rename.

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp).map_err(|e| anyerr!("writing {path:?}: {e}"))?;
    f.write_all(bytes)
        .map_err(|e| anyerr!("writing {path:?}: {e}"))?;
    f.sync_all().map_err(|e| anyerr!("writing {path:?}: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| anyerr!("writing {path:?}: {e}"))?;
    if let Some(dir) = path.parent() {
        fsync_dir(dir).map_err(|e| anyerr!("writing {path:?}: {e}"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn fsync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Rebuilds a [`StorageKey`] from its `components()` strings. The crate's
/// `TryFrom<Vec<String>>` drops a component and maps namespaces to `Other`,
/// hence this by-shape rebuild.
fn key_from_components(parts: &[String]) -> Result<StorageKey> {
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["blobs", hash] => Ok(StorageKey::blob(
            hash.parse().map_err(|e| anyerr!("bad blob key: {e}"))?,
        )),
        ["sedimentrees", doc, category, rest @ ..] => {
            let category = match *category {
                "content" => CommitCategory::Content,
                "index" => CommitCategory::Index,
                other => return Err(anyerr!("unknown commit category {other}")),
            };
            let doc = doc
                .parse()
                .map_err(|e| anyerr!("bad doc id in storage key: {e}"))?;
            let mut key = StorageKey::sedimentree_root(&doc, category);
            for part in rest {
                key = key.with_subcomponent(part);
            }
            Ok(key)
        }
        other => Err(anyerr!("unknown storage key shape {other:?}")),
    }
}

/// The document a sedimentree key belongs to; `None` for blob/other keys.
fn key_doc(key: &StorageKey) -> Option<DocumentId> {
    let mut parts = key.components();
    match (parts.next(), parts.next()) {
        (Some("sedimentrees"), Some(doc)) => doc.parse().ok(),
        _ => None,
    }
}

/// The hash (lowercase hex) of a blob storage key; `None` for other keys.
/// Both beelay's `BlobHash` and iroh-blobs' `Hash` render as the same
/// 64-char lowercase hex, so one string keyspace covers both.
fn key_blob(key: &StorageKey) -> Option<String> {
    let mut parts = key.components();
    match (parts.next(), parts.next()) {
        (Some("blobs"), Some(hash)) => Some(hash.to_owned()),
        _ => None,
    }
}

/// Owns kv.bin: [`Core::persist_kv`] publishes whole-map snapshots into `tx`;
/// one writer task writes them in order, newest snapshot wins.
struct KvWriter {
    tx: tokio::sync::watch::Sender<Vec<u8>>,
    task: tokio::task::JoinHandle<()>,
}

impl KvWriter {
    fn spawn(path: PathBuf) -> Self {
        let (tx, mut rx) = tokio::sync::watch::channel(Vec::new());
        let task = tokio::spawn(async move {
            loop {
                let open = rx.changed().await.is_ok();
                let bytes = rx.borrow_and_update().clone();
                if !bytes.is_empty() {
                    let path = path.clone();
                    match tokio::task::spawn_blocking(move || write_file(&path, &bytes)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => eprintln!("beelay: persisting kv.bin failed: {e}"),
                        Err(e) => eprintln!("beelay: kv write task failed: {e}"),
                    }
                }
                if !open {
                    return;
                }
            }
        });
        Self { tx, task }
    }
}

/// The sans-IO beelay state machine plus its synchronously-answered storage.
struct Core {
    beelay: Beelay<OsRng>,
    // vendor-edit: live map answers IoTasks; every Put/Delete is mirrored to
    // disk through the kv writer (when set) so synced docs survive a restart.
    storage: BTreeMap<StorageKey, Vec<u8>>,
    kv: Option<KvWriter>,
    /// Set by Put/Delete; [`Core::drive_scoped`] snapshots once per pump.
    dirty: bool,
    // vendor-edit: blob hash (lowercase hex) -> owning beelay doc, one
    // keyspace for beelay commit blobs and iroh PDF blobs (spec §4). Scoped
    // drives and the blobs ALPN gate consult it so blob data scopes per
    // project. Persisted as blob_docs.bin beside the registry.
    blob_docs: HashMap<String, DocumentId>,
    blob_kv: Option<KvWriter>,
    /// Set on blob_docs changes; persisted like `dirty`.
    blob_dirty: bool,
    stories: HashMap<StoryId, StoryResult>,
}

impl Core {
    fn new(peer_id: PeerId, data_dir: Option<&Path>) -> Result<Self> {
        let kv_path = data_dir.map(|d| d.join("kv.bin"));
        let blob_path = data_dir.map(|d| d.join("blob_docs.bin"));
        let mut storage = BTreeMap::new();
        if let Some(path) = &kv_path
            && path.exists()
        {
            let bytes = std::fs::read(path).map_err(|e| anyerr!("reading beelay kv: {e}"))?;
            match postcard::from_bytes::<Vec<(Vec<String>, Vec<u8>)>>(&bytes) {
                Ok(entries) => {
                    for (parts, value) in entries {
                        match key_from_components(&parts) {
                            Ok(key) => {
                                storage.insert(key, value);
                            }
                            Err(e) => eprintln!("beelay: skipping kv entry {parts:?}: {e}"),
                        }
                    }
                }
                Err(e) => eprintln!("beelay: malformed kv.bin, starting empty: {e}"),
            }
        }
        let mut blob_docs = HashMap::new();
        if let Some(path) = &blob_path
            && path.exists()
        {
            let bytes = std::fs::read(path).map_err(|e| anyerr!("reading blob map: {e}"))?;
            match postcard::from_bytes::<Vec<(String, [u8; 16])>>(&bytes) {
                Ok(entries) => {
                    blob_docs = entries
                        .into_iter()
                        .map(|(hash, doc)| (hash, DocumentId::from(doc)))
                        .collect();
                }
                Err(e) => eprintln!("beelay: malformed blob_docs.bin, starting empty: {e}"),
            }
        }
        Ok(Self {
            beelay: Beelay::new(peer_id, OsRng),
            storage,
            kv: kv_path.map(KvWriter::spawn),
            dirty: false,
            blob_docs,
            blob_kv: blob_path.map(KvWriter::spawn),
            blob_dirty: false,
            stories: HashMap::new(),
        })
    }

    /// A session-local, memory-only core over a snapshot of `storage` (and
    /// the blob map that scopes it): serves the snapshot's commits under the
    /// same doc ids (beelay has no doc registry — storage keys ARE the doc),
    /// and everything a peer uploads into it evaporates on drop (`kv: None`,
    /// nothing reaches disk). Serve-only Read-role sessions.
    fn scratch(
        peer_id: PeerId,
        storage: BTreeMap<StorageKey, Vec<u8>>,
        blob_docs: HashMap<String, DocumentId>,
    ) -> Self {
        Self {
            beelay: Beelay::new(peer_id, OsRng),
            storage,
            kv: None,
            dirty: false,
            blob_docs,
            blob_kv: None,
            blob_dirty: false,
            stories: HashMap::new(),
        }
    }

    // ponytail: whole-map snapshot per dirty drive, sent to the single kv
    // writer task; per-key files when docs grow.
    fn persist_kv(&self) -> Result<()> {
        let Some(kv) = &self.kv else {
            return Ok(());
        };
        let entries: Vec<(Vec<String>, &Vec<u8>)> = self
            .storage
            .iter()
            .map(|(k, v)| (k.components().map(str::to_owned).collect(), v))
            .collect();
        let bytes =
            postcard::to_stdvec(&entries).map_err(|e| anyerr!("encoding beelay kv: {e}"))?;
        kv.tx.send_replace(bytes);
        Ok(())
    }

    fn persist_blob_docs(&self) -> Result<()> {
        let Some(kv) = &self.blob_kv else {
            return Ok(());
        };
        let entries: Vec<(&String, [u8; 16])> = self
            .blob_docs
            .iter()
            .map(|(hash, doc)| (hash, *doc.as_bytes()))
            .collect();
        let bytes =
            postcard::to_stdvec(&entries).map_err(|e| anyerr!("encoding blob map: {e}"))?;
        kv.tx.send_replace(bytes);
        Ok(())
    }

    /// Feeds `event` through the state machine, answering every IoTask from
    /// the KV until quiescent. Returns outbound envelopes; completed stories
    /// land in `self.stories`.
    fn drive(&mut self, event: Event) -> Result<Vec<Envelope>> {
        self.drive_scoped(event, None)
    }

    /// [`Self::drive`] with storage access confined to one document: loads
    /// outside `scope`'s sedimentree answer empty and writes are dropped.
    fn drive_scoped(&mut self, event: Event, scope: Option<DocumentId>) -> Result<Vec<Envelope>> {
        let mut inbox = VecDeque::from([event]);
        let mut out = Vec::new();
        while let Some(event) = inbox.pop_front() {
            let results = self
                .beelay
                .handle_event(event)
                .map_err(|e| anyerr!("beelay: {e}"))?;
            out.extend(results.new_messages);
            for task in results.new_tasks {
                inbox.push_back(Event::io_complete(self.io(task, scope)?));
            }
            self.stories.extend(results.completed_stories);
            // notifications only come from `listen` stories, which we never
            // start — sessions are one-shot pulls like Phase 1.
        }
        if self.dirty {
            self.persist_kv()?;
            self.dirty = false;
        }
        if self.blob_dirty {
            self.persist_blob_docs()?;
            self.blob_dirty = false;
        }
        Ok(out)
    }

    fn io(&mut self, task: IoTask, scope: Option<DocumentId>) -> Result<IoResult> {
        // vendor-edit: blob keys scope through the blob -> doc map (spec §4):
        // in a scoped drive a blob mapped to another doc — or not mapped at
        // all — is invisible, so a peer syncing doc Q can't pull doc P's
        // commit blobs by hash. Unmapped means stored before the map existed;
        // the flagged layer has no real deployments, so denying needs no
        // legacy-blob migration. Unscoped (local) drives stay unrestricted.
        fn in_scope(
            blob_docs: &HashMap<String, DocumentId>,
            scope: Option<DocumentId>,
            key: &StorageKey,
        ) -> bool {
            let Some(scope) = scope else { return true };
            if let Some(doc) = key_doc(key) {
                return doc == scope;
            }
            match key_blob(key) {
                Some(hash) => blob_docs.get(&hash) == Some(&scope),
                None => true,
            }
        }
        let id = task.id();
        Ok(match task.take_action() {
            IoAction::Load { key } => IoResult::load(
                id,
                in_scope(&self.blob_docs, scope, &key)
                    .then(|| self.storage.get(&key).cloned())
                    .flatten(),
            ),
            IoAction::LoadRange { prefix } => IoResult::load_range(
                id,
                self.storage
                    .iter()
                    .filter(|(k, _)| prefix.is_prefix_of(k) && in_scope(&self.blob_docs, scope, k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            IoAction::Put { key, data } => {
                // a scoped Put of a not-yet-mapped blob claims it for the
                // scope doc (an already-claimed blob keeps its owner).
                if let Some(doc) = scope
                    && let Some(hash) = key_blob(&key)
                    && !self.blob_docs.contains_key(&hash)
                {
                    self.blob_docs.insert(hash, doc);
                    self.blob_dirty = true;
                }
                if in_scope(&self.blob_docs, scope, &key) {
                    self.storage.insert(key, data);
                    self.dirty = true;
                }
                IoResult::put(id)
            }
            IoAction::Delete { key } => {
                if in_scope(&self.blob_docs, scope, &key) {
                    if let Some(hash) = key_blob(&key)
                        && self.blob_docs.remove(&hash).is_some()
                    {
                        self.blob_dirty = true;
                    }
                    self.storage.remove(&key);
                    self.dirty = true;
                }
                IoResult::delete(id)
            }
            // pure p2p: never forward requests for a doc to other peers.
            IoAction::Ask { .. } => IoResult::ask(id, HashSet::new()),
        })
    }

    /// Result of a story that completes without network round trips.
    fn take_story(&mut self, id: StoryId) -> Result<StoryResult> {
        self.stories
            .remove(&id)
            .ok_or_else(|| anyerr!("beelay story did not complete synchronously"))
    }
}

// --- node state ------------------------------------------------------------------

/// Where a project's canonical copy lives. Three-state (write-enforcement
/// spec §5) so a member whose stored host ticket failed to parse never
/// presents like a host.
enum ProjectHost {
    /// This node hosts the project; members dial us.
    Hosted,
    /// Member: the host address [`BeelayNode::sync_project`] dials; set by
    /// `accept_invite`.
    Member(EndpointAddr),
    /// Member whose registry host ticket was unreadable at load;
    /// [`BeelayNode::sync_project`] fails with [`HostTicketError`] until a
    /// re-accepted invite repairs the address.
    MemberBadTicket,
}

struct ProjectState {
    /// The local plaintext document.
    doc: Automerge,
    beelay_doc: DocumentId,
    host: ProjectHost,
    /// automerge change hash -> beelay commit hash (commit = sealed change).
    change_to_commit: HashMap<ChangeHash, CommitHash>,
    /// Commits already applied to `doc` (or produced from it).
    applied: HashSet<CommitHash>,
    /// Set when a refresh failed and `doc` may be missing history; flush
    /// refuses until a later refresh succeeds and clears it.
    unhealthy: bool,
}

struct State {
    core: Core,
    projects: HashMap<String, ProjectState>,
}

struct Shared {
    auth: ProjectAuth,
    peer_id: PeerId,
    /// This device's cross-signed (iroh, keyhive) binding, sent in every
    /// keyhive preamble hello.
    binding: DeviceBinding,
    // vendor-edit: project registry file (id -> beelay doc + host), so a
    // restart can rebuild `State.projects` and re-decrypt from the KV.
    registry_path: Option<PathBuf>,
    // registry writes happen off the state lock; this lock keeps them one at
    // a time (it is acquired before the state guard drops).
    registry_write: Mutex<()>,
    // endpoint -> keyhive member, learned from binding-verified preambles and
    // persisted with the registry; the blobs gate resolves dialers through it.
    peers: std::sync::Mutex<HashMap<EndpointId, MemberId>>,
    // Beelay is Send-not-Sync; the tokio Mutex makes it shareable. The lock is
    // never held across a network await — only across local compute (including
    // keyhive encrypt/decrypt).
    state: Mutex<State>,
}

// vendor-edit: registry.bin host field. On-disk compat is deliberate (spec
// §5 choice (a)): the variant order is load-bearing — postcard encodes
// `Hosted` as varint discriminant 0 and `Member` as varint 1 + string,
// byte-identical to the pre-3-state `Option<String>` layout (None = 0x00,
// Some = 0x01 + payload; verified against postcard 1.1's
// serialize_none/serialize_unit_variant/serialize_newtype_variant), so old
// registry files load unchanged. `MemberBadTicket` (2) never appears in old
// files.
#[derive(serde::Serialize, serde::Deserialize)]
enum RegistryHost {
    Hosted,
    Member(String),
    MemberBadTicket,
}

/// registry.bin entry: (project id, beelay doc bytes, host).
type RegistryEntry = (String, [u8; 16], RegistryHost);
/// registry.bin payload: project entries plus the peer map
/// (endpoint id bytes -> member id bytes).
type RegistryFile = (Vec<RegistryEntry>, Vec<([u8; 32], [u8; 32])>);

impl Shared {
    async fn project_ids(&self) -> Vec<String> {
        self.state.lock().await.projects.keys().cloned().collect()
    }

    /// Serializes the registry under the state lock, then consumes the guard
    /// and writes on the blocking pool.
    async fn persist_registry(&self, state: tokio::sync::MutexGuard<'_, State>) -> Result<()> {
        let Some(path) = &self.registry_path else {
            return Ok(());
        };
        let entries: Vec<RegistryEntry> = state
            .projects
            .iter()
            .map(|(id, p)| {
                let host = match &p.host {
                    ProjectHost::Hosted => RegistryHost::Hosted,
                    ProjectHost::Member(addr) => {
                        RegistryHost::Member(EndpointTicket::new(addr.clone()).encode_string())
                    }
                    ProjectHost::MemberBadTicket => RegistryHost::MemberBadTicket,
                };
                (id.clone(), *p.beelay_doc.as_bytes(), host)
            })
            .collect();
        let peers: Vec<([u8; 32], [u8; 32])> = self
            .peers
            .lock()
            .unwrap()
            .iter()
            .map(|(endpoint, member)| (*endpoint.as_bytes(), member.0))
            .collect();
        let bytes = postcard::to_stdvec(&(entries, peers))
            .map_err(|e| anyerr!("encoding project registry: {e}"))?;
        let path = path.clone();
        let write_guard = self.registry_write.lock().await;
        drop(state);
        let result = tokio::task::spawn_blocking(move || write_file(&path, &bytes))
            .await
            .map_err(|e| anyerr!("registry write task: {e}"))?;
        drop(write_guard);
        result
    }

    /// Encrypts every not-yet-flushed local change into a beelay commit.
    async fn flush(&self, project_id: &str) -> Result<()> {
        let mut guard = self.state.lock().await;
        let state = &mut *guard;
        let proj = state
            .projects
            .get_mut(project_id)
            .ok_or_else(|| anyerr!("unknown project {project_id}"))?;
        if proj.unhealthy {
            return Err(anyerr!(
                "project {project_id} failed its last refresh; sync it before flushing"
            ));
        }
        let pending = unflushed_changes(&proj.doc, &proj.change_to_commit)?;
        if pending.is_empty() {
            return Ok(());
        }
        let mut commits = Vec::with_capacity(pending.len());
        for change in pending {
            let parents = commit_parents(&change, &proj.change_to_commit)?;
            let sealed = self.auth.encrypt(project_id, change.raw_bytes()).await?;
            let hash = CommitHash::from(*blake3::hash(&sealed).as_bytes());
            proj.change_to_commit.insert(change.hash(), hash);
            proj.applied.insert(hash);
            commits.push(Commit::new(parents, sealed, hash));
        }
        let beelay_doc = proj.beelay_doc;
        let (story, event) = Event::add_commits(beelay_doc, commits);
        // scoped so the commits' blob puts are claimed for this doc in the
        // blob -> doc map (spec §4); add_commits touches only this doc.
        state.core.drive_scoped(event, Some(beelay_doc))?;
        // ponytail: AddCommits returns BundleSpecs when a hash lands on a
        // strata boundary; we never bundle — loose commits sync fine at
        // project scale. Build CommitBundles here if histories grow long.
        let StoryResult::AddCommits(_bundle_specs) = state.core.take_story(story)? else {
            return Err(anyerr!("unexpected story result for add_commits"));
        };
        Ok(())
    }

    /// Decrypts and applies every new commit in beelay storage to the local
    /// doc, rebuilding the change->commit map from the plaintext changes.
    async fn refresh(&self, project_id: &str) -> Result<SyncOutcome> {
        let mut guard = self.state.lock().await;
        let state = &mut *guard;
        let beelay_doc = state
            .projects
            .get(project_id)
            .ok_or_else(|| anyerr!("unknown project {project_id}"))?
            .beelay_doc;
        let (story, event) = Event::load_doc(beelay_doc);
        state.core.drive(event)?;
        let StoryResult::LoadDoc(commits) = state.core.take_story(story)? else {
            return Err(anyerr!("unexpected story result for load_doc"));
        };
        let proj = state
            .projects
            .get_mut(project_id)
            .ok_or_else(|| anyerr!("project {project_id} vanished"))?;
        let mut outcome = SyncOutcome::default();
        let mut changes = Vec::new();
        // commit hash -> change hash, held back until apply_changes succeeds:
        // marking a commit applied before the doc actually contains it would
        // make every later refresh skip it forever if apply fails.
        let mut pending: HashMap<CommitHash, ChangeHash> = HashMap::new();
        for item in commits.unwrap_or_default() {
            // ponytail: we never create bundles, so only loose commits exist;
            // handle CommitOrBundle::Bundle when bundling lands.
            let CommitOrBundle::Commit(commit) = item else {
                continue;
            };
            if proj.applied.contains(&commit.hash()) || pending.contains_key(&commit.hash()) {
                continue;
            }
            match self.auth.decrypt(project_id, commit.contents()).await {
                Ok(plain) => {
                    let change = Change::from_bytes(plain)
                        .map_err(|e| anyerr!("decrypted commit is not an automerge change: {e}"))?;
                    pending.insert(commit.hash(), change.hash());
                    changes.push(change);
                }
                // undecryptable commits stay unapplied and are retried on the
                // next refresh (a later event ingest may unlock them).
                Err(err) => outcome.undecryptable.push(err),
            }
        }
        outcome.applied = changes.len();
        proj.doc
            .apply_changes(changes)
            .map_err(|e| anyerr!("applying synced changes: {e}"))?;
        for (commit_hash, change_hash) in pending {
            proj.change_to_commit.insert(change_hash, commit_hash);
            proj.applied.insert(commit_hash);
        }
        proj.unhealthy = false;
        Ok(outcome)
    }
}

// --- framing (tag byte inside Phase 1's 8-byte LE length frames) ---------------

async fn send_tagged(send: &mut SendStream, tag: u8, body: &[u8]) -> Result<()> {
    let mut buf = Vec::with_capacity(body.len() + 1);
    buf.push(tag);
    buf.extend_from_slice(body);
    send_frame(send, &buf).await
}

/// `None` = the peer's zero-length end-of-session frame.
async fn recv_tagged(recv: &mut RecvStream) -> Result<Option<(u8, Vec<u8>)>> {
    match recv_frame(recv).await? {
        None => Ok(None),
        Some(mut buf) => {
            let tag = buf[0]; // nonempty: recv_frame returns None for len 0
            buf.remove(0); // ponytail: O(n) shift, frames are small
            Ok(Some((tag, buf)))
        }
    }
}

async fn expect_tagged(recv: &mut RecvStream, want: u8) -> Result<Vec<u8>> {
    match recv_tagged(recv).await? {
        Some((tag, body)) if tag == want => Ok(body),
        Some((tag, _)) => Err(anyerr!("expected frame tag {want}, peer sent {tag}")),
        None => Err(anyerr!("peer ended the session early")),
    }
}

// --- session building blocks ---------------------------------------------------

/// Cap on each keyhive preamble frame, far below the generic 64 MiB frame
/// cap: this runs before any identity verification, and keyhive event
/// ingestion is an O(n^2)-worst-case fixed-point loop — an unauthenticated
/// dialer must not be able to feed it hundreds of thousands of events.
// ponytail: 1 MiB ~ thousands of membership events; bump if a project's
// delegation history legitimately outgrows it.
const MAX_KEYHIVE_FRAME: usize = 1024 * 1024;

/// Cap on the first (project-id) frame of a session, read before any
/// identity verification; project ids are short strings.
const MAX_PROJECT_ID_FRAME: usize = 512;

async fn expect_keyhive(recv: &mut RecvStream) -> Result<Vec<u8>> {
    let body = expect_tagged(recv, TAG_KEYHIVE).await?;
    if body.len() > MAX_KEYHIVE_FRAME {
        return Err(anyerr!(
            "oversized keyhive preamble frame ({} bytes)",
            body.len()
        ));
    }
    Ok(body)
}

/// Decodes and checks a preamble hello: the binding's signatures must
/// verify, its endpoint must be the TLS-authenticated `remote`, and the
/// contact card must belong to the bound keyhive member.
async fn verify_peer_hello(
    auth: &ProjectAuth,
    remote: EndpointId,
    frame: &[u8],
) -> Result<MemberId> {
    let (binding, card): (DeviceBinding, Vec<u8>) =
        postcard::from_bytes(frame).map_err(|e| anyerr!("malformed preamble hello: {e}"))?;
    binding.verify()?;
    let bound = binding.endpoint_id()?;
    if bound != remote {
        return Err(anyerr!(
            "device binding endpoint {bound} does not match connection remote {remote}"
        ));
    }
    let peer = auth.receive_contact_card(&card).await?;
    if peer != binding.member_id() {
        return Err(anyerr!("contact card member does not match device binding"));
    }
    Ok(peer)
}

/// Both sides swap hellos (device binding + contact card), then swap +
/// ingest each other's keyhive static events, so CGKA ops arrive before any
/// beelay traffic. `initiate` picks send-first vs receive-first (like
/// `run_sync`): a symmetric write-then-read on both ends deadlocks once the
/// event payloads outgrow the QUIC stream flow-control window, since each
/// side's write waits for a read the other side never starts.
async fn keyhive_preamble(
    auth: &ProjectAuth,
    binding: &DeviceBinding,
    remote: EndpointId,
    send: &mut SendStream,
    recv: &mut RecvStream,
    initiate: bool,
) -> Result<MemberId> {
    let hello = postcard::to_stdvec(&(binding, auth.contact_card().await?))
        .map_err(|e| anyerr!("encoding preamble hello: {e}"))?;
    let peer;
    let events;
    if initiate {
        send_tagged(send, TAG_KEYHIVE, &hello).await?;
        peer = verify_peer_hello(auth, remote, &expect_keyhive(recv).await?).await?;
        send_tagged(send, TAG_KEYHIVE, &auth.export_events_for(peer).await?).await?;
        events = expect_keyhive(recv).await?;
    } else {
        peer = verify_peer_hello(auth, remote, &expect_keyhive(recv).await?).await?;
        send_tagged(send, TAG_KEYHIVE, &hello).await?;
        events = expect_keyhive(recv).await?;
        send_tagged(send, TAG_KEYHIVE, &auth.export_events_for(peer).await?).await?;
    }
    // ponytail: stuck events tolerated — post-revocation exports are
    // legitimately partial (Phase 2's revoke test shape). Tighten to a hard
    // error once exports are always dependency-complete.
    let _ = auth.ingest_events(&events).await;
    Ok(peer)
}

/// Rejects a hello whose announced beelay PeerId is not the string form of
/// the TLS-authenticated iroh endpoint id on this connection.
fn verify_hello(connected: &Connected, remote: EndpointId) -> Result<()> {
    if connected.their_peer_id().to_string() != remote.to_string() {
        return Err(anyerr!(
            "beelay hello peer id {} does not match connection remote {remote}",
            connected.their_peer_id()
        ));
    }
    Ok(())
}

async fn handshake_connect(
    us: PeerId,
    remote: EndpointId,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<Connected> {
    let Step::Continue(pending, Some(hello)) = Connecting::connect(us) else {
        return Err(anyerr!("beelay connect handshake produced no hello"));
    };
    send_tagged(send, TAG_BEELAY, &hello.encode()).await?;
    let reply = expect_tagged(recv, TAG_BEELAY).await?;
    let reply = Message::decode(&reply).std_context("decoding handshake reply")?;
    let Step::Done(connected, None) = pending.receive(reply).std_context("beelay handshake")?
    else {
        return Err(anyerr!("beelay handshake did not finish in one round trip"));
    };
    verify_hello(&connected, remote)?;
    Ok(connected)
}

async fn handshake_accept(
    us: PeerId,
    remote: EndpointId,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<Connected> {
    let Step::Continue(pending, None) = Connecting::accept(us) else {
        return Err(anyerr!("beelay accept handshake wanted to speak first"));
    };
    let hello = expect_tagged(recv, TAG_BEELAY).await?;
    let hello = Message::decode(&hello).std_context("decoding handshake hello")?;
    let Step::Done(connected, Some(reply)) =
        pending.receive(hello).std_context("beelay handshake")?
    else {
        return Err(anyerr!("beelay handshake did not finish on hello"));
    };
    // verify BEFORE replying: a spoofed hello never gets a response.
    verify_hello(&connected, remote)?;
    send_tagged(send, TAG_BEELAY, &reply.encode()).await?;
    Ok(connected)
}

async fn send_envelopes(
    connected: &Connected,
    envelopes: Vec<Envelope>,
    send: &mut SendStream,
) -> Result<()> {
    for env in envelopes {
        // single-connection topology: beelay only addresses the peer it is
        // syncing with (Ask answers an empty forward set), so anything else
        // is a bug — never misdeliver.
        if env.recipient() != connected.their_peer_id() {
            return Err(anyerr!(
                "beelay addressed {} on a connection to {}",
                env.recipient(),
                connected.their_peer_id()
            ));
        }
        send_tagged(send, TAG_BEELAY, &connected.send(env).encode()).await?;
    }
    Ok(())
}

// --- node ------------------------------------------------------------------------

/// An encrypted-share node: iroh transport + beelay sync engine + keyhive
/// capability layer + a registry of plaintext automerge project docs.
///
/// Flow: [`Self::create_shared_project`] -> exchange contact cards +
/// [`ProjectAuth::add_member`] -> [`Self::invite`] -> peer
/// [`Self::accept_invite`] + [`Self::sync_project`]. Revoke via
/// [`ProjectAuth::revoke_member`] (rotates the key), confirm with
/// [`ProjectAuth::query_access`].
pub struct BeelayNode {
    router: Router,
    shared: Arc<Shared>,
    // vendor-edit: FsStore under data_dir/blobs when a data dir is given,
    // in-memory otherwise (tests, throwaway nodes).
    blobs: Store,
}

impl fmt::Debug for BeelayNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BeelayNode({})", self.endpoint_id())
    }
}

impl BeelayNode {
    /// Binds with n0 discovery + relays: dialable by bare [`EndpointId`].
    /// `data_dir` = where beelay commits, the project registry, and blobs
    /// persist; `None` keeps everything in memory (lost on drop).
    pub async fn bind(
        identity: &DeviceIdentity,
        auth_identity: &AuthIdentity,
        auth: ProjectAuth,
        data_dir: Option<&Path>,
    ) -> Result<Self> {
        Self::bind_with(identity, auth_identity, auth, data_dir, presets::N0).await
    }

    /// Binds without discovery or relays: peers must dial the full
    /// [`EndpointAddr`] carried in invites. Offline/LAN use and tests.
    pub async fn bind_local(
        identity: &DeviceIdentity,
        auth_identity: &AuthIdentity,
        auth: ProjectAuth,
        data_dir: Option<&Path>,
    ) -> Result<Self> {
        Self::bind_with(identity, auth_identity, auth, data_dir, presets::Minimal).await
    }

    async fn bind_with(
        identity: &DeviceIdentity,
        auth_identity: &AuthIdentity,
        auth: ProjectAuth,
        data_dir: Option<&Path>,
        preset: impl presets::Preset,
    ) -> Result<Self> {
        let endpoint = Endpoint::builder(preset)
            .secret_key(identity.secret().clone())
            .bind()
            .await
            .context("binding iroh endpoint")?;
        let binding = DeviceBinding::create(identity, auth_identity);
        let (shared, blobs) = Self::prepare(endpoint.id(), auth, binding, data_dir).await?;
        let router = Router::builder(endpoint)
            .accept(
                BEELAY_ALPN,
                BeelayProtocol {
                    shared: shared.clone(),
                },
            )
            .accept(
                iroh_blobs::ALPN,
                GatedBlobs {
                    inner: BlobsProtocol::new(&blobs, None),
                    shared: shared.clone(),
                },
            )
            .spawn();
        Self::finish(router, shared, blobs).await
    }

    // vendor-edit: bind split into prepare (state + stores from disk) and
    // finish (post-router doc refresh) so bind_stack can mount the beelay and
    // blobs protocols on a router shared with plain sync.
    async fn prepare(
        endpoint_id: EndpointId,
        auth: ProjectAuth,
        binding: DeviceBinding,
        data_dir: Option<&Path>,
    ) -> Result<(Arc<Shared>, Store)> {
        let peer_id = PeerId::from(endpoint_id.to_string());
        if let Some(dir) = data_dir {
            std::fs::create_dir_all(dir).map_err(|e| anyerr!("creating {dir:?}: {e}"))?;
        }
        let core = Core::new(peer_id.clone(), data_dir)?;
        // ponytail: docs rebuilt from encrypted commits below — plaintext
        // edits not yet flushed (encrypted at invite/sync time) do not
        // survive a restart; flush on shutdown if that ever bites.
        let mut projects = HashMap::new();
        let mut peers = HashMap::new();
        let registry_path = data_dir.map(|d| d.join("registry.bin"));
        if let Some(path) = &registry_path
            && path.exists()
        {
            let bytes =
                std::fs::read(path).map_err(|e| anyerr!("reading project registry: {e}"))?;
            match postcard::from_bytes::<RegistryFile>(&bytes) {
                Ok((entries, peer_entries)) => {
                    for (id, doc, host) in entries {
                        // a bad ticket becomes MemberBadTicket, NOT Hosted:
                        // sync_project surfaces it typed, and a re-accepted
                        // invite repairs the address.
                        let host = match host {
                            RegistryHost::Hosted => ProjectHost::Hosted,
                            RegistryHost::Member(t) => match EndpointTicket::decode_string(&t) {
                                Ok(ticket) => ProjectHost::Member(ticket.endpoint_addr().clone()),
                                Err(e) => {
                                    eprintln!("beelay: bad host ticket in registry for {id}: {e}");
                                    ProjectHost::MemberBadTicket
                                }
                            },
                            RegistryHost::MemberBadTicket => ProjectHost::MemberBadTicket,
                        };
                        projects.insert(
                            id,
                            ProjectState {
                                doc: Automerge::new(),
                                beelay_doc: DocumentId::from(doc),
                                host,
                                change_to_commit: HashMap::new(),
                                applied: HashSet::new(),
                                unhealthy: false,
                            },
                        );
                    }
                    for (endpoint, member) in peer_entries {
                        match EndpointId::from_bytes(&endpoint) {
                            Ok(id) => {
                                peers.insert(id, MemberId(member));
                            }
                            Err(e) => eprintln!("beelay: skipping registry peer entry: {e}"),
                        }
                    }
                }
                Err(e) => eprintln!("beelay: malformed project registry, starting empty: {e}"),
            }
        }
        let shared = Arc::new(Shared {
            auth,
            peer_id,
            binding,
            registry_path,
            registry_write: Mutex::new(()),
            peers: std::sync::Mutex::new(peers),
            state: Mutex::new(State { core, projects }),
        });
        let blobs: Store = match data_dir {
            Some(dir) => FsStore::load(dir.join("blobs"))
                .await
                .map_err(|e| anyerr!("loading blob store: {e}"))?
                .into(),
            None => MemStore::new().into(),
        };
        Ok((shared, blobs))
    }

    async fn finish(router: Router, shared: Arc<Shared>, blobs: Store) -> Result<Self> {
        let node = Self {
            router,
            shared,
            blobs,
        };
        // decrypt persisted commits back into the plaintext docs; commits
        // from epochs this device can't read stay pending, like any refresh.
        for id in node.shared.project_ids().await {
            if let Err(e) = node.shared.refresh(&id).await {
                eprintln!("beelay: startup refresh of project {id} failed: {e}");
                let mut state = node.shared.state.lock().await;
                if let Some(proj) = state.projects.get_mut(&id) {
                    proj.unhealthy = true;
                }
            }
        }
        Ok(node)
    }

    /// This node's share identity.
    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }

    /// This node's current dialable address.
    pub fn addr(&self) -> EndpointAddr {
        self.router.endpoint().addr()
    }

    /// The capability layer, for contact cards, grants, revocation, and
    /// access queries.
    pub fn auth(&self) -> &ProjectAuth {
        &self.shared.auth
    }

    /// Registers `doc` as a new shared project: creates the keyhive
    /// group/doc and an empty beelay doc. Content is NOT encrypted here —
    /// changes are flushed lazily at invite/sync time so they land in an
    /// epoch every current member can read (grant-before-encrypt ordering;
    /// upstream keyhive #136 makes pre-grant ciphertext unreadable to
    /// later-added members).
    pub async fn create_shared_project(&self, project_id: &str, doc: Automerge) -> Result<()> {
        self.shared.auth.create_project(project_id).await?;
        let mut state = self.shared.state.lock().await;
        let (story, event) = Event::create_doc();
        state.core.drive(event)?;
        let StoryResult::CreateDoc(beelay_doc) = state.core.take_story(story)? else {
            return Err(anyerr!("unexpected story result for create_doc"));
        };
        state.projects.insert(
            project_id.to_owned(),
            ProjectState {
                doc,
                beelay_doc,
                host: ProjectHost::Hosted,
                change_to_commit: HashMap::new(),
                applied: HashSet::new(),
                unhealthy: false,
            },
        );
        self.shared.persist_registry(state).await?;
        Ok(())
    }

    /// A pasteable invite for `member`, who must already hold a role from
    /// [`ProjectAuth::add_member`]. Pending local changes are encrypted now,
    /// after the grant, so the invitee can decrypt everything it will fetch.
    pub async fn invite(&self, project_id: &str, member: MemberId) -> Result<String> {
        self.shared.flush(project_id).await?;
        let events = self.shared.auth.export_events_for(member).await?;
        let keyhive_doc = self
            .shared
            .auth
            .doc_id(project_id)
            .ok_or_else(|| anyerr!("unknown project {project_id}"))?;
        let group = self
            .shared
            .auth
            .group_id(project_id)
            .ok_or_else(|| anyerr!("no membership group known for {project_id}"))?;
        let beelay_doc = {
            let state = self.shared.state.lock().await;
            let proj = state
                .projects
                .get(project_id)
                .ok_or_else(|| anyerr!("unknown project {project_id}"))?;
            *proj.beelay_doc.as_bytes()
        };
        let invite = ProjectInvite {
            endpoint: EndpointTicket::new(self.router.endpoint().addr()),
            project_id: project_id.to_owned(),
            beelay_doc,
            keyhive_doc,
            group,
            events,
        };
        Ok(invite.encode_string())
    }

    /// Ingests an invite: learns the delegations, adopts the keyhive doc,
    /// and registers an empty local doc wired to the host's address. Returns
    /// the project id; call [`Self::sync_project`] to fetch the content.
    /// Re-accepting a known project (retry, rejoin after leave) re-ingests
    /// the events and refreshes the host address without touching its doc.
    pub async fn accept_invite(&self, invite: &str) -> Result<String> {
        let invite: ProjectInvite = invite
            .parse()
            .map_err(|e| anyerr!("malformed invite: {e}"))?;
        // the lock is held across check + ingest + adopt + insert
        // (ingest/adopt are local keyhive compute — no network awaits).
        let mut state = self.shared.state.lock().await;
        self.shared.auth.ingest_events(&invite.events).await?;
        if let Some(proj) = state.projects.get_mut(&invite.project_id) {
            if matches!(proj.host, ProjectHost::Hosted) {
                return Err(anyerr!(
                    "this node hosts project {}; refusing an invite for it",
                    invite.project_id
                ));
            }
            if DocumentId::from(invite.beelay_doc) != proj.beelay_doc {
                return Err(anyerr!(
                    "invite beelay doc does not match known project {}",
                    invite.project_id
                ));
            }
            match self.shared.auth.doc_id(&invite.project_id) {
                Some(doc) if doc != invite.keyhive_doc => {
                    return Err(anyerr!(
                        "invite keyhive doc does not match known project {}",
                        invite.project_id
                    ));
                }
                Some(_) => {}
                None => {
                    self.shared
                        .auth
                        .adopt_project(&invite.project_id, invite.keyhive_doc, Some(invite.group))
                        .await?;
                }
            }
            proj.host = ProjectHost::Member(invite.endpoint.endpoint_addr().clone());
        } else {
            self.shared
                .auth
                .adopt_project(&invite.project_id, invite.keyhive_doc, Some(invite.group))
                .await?;
            state.projects.insert(
                invite.project_id.clone(),
                ProjectState {
                    doc: Automerge::new(),
                    beelay_doc: DocumentId::from(invite.beelay_doc),
                    host: ProjectHost::Member(invite.endpoint.endpoint_addr().clone()),
                    change_to_commit: HashMap::new(),
                    applied: HashSet::new(),
                    unhealthy: false,
                },
            );
        }
        self.shared.persist_registry(state).await?;
        Ok(invite.project_id)
    }

    /// Runs `f` on the local plaintext document. `None` if unknown.
    pub async fn with_doc<T>(
        &self,
        project_id: &str,
        f: impl FnOnce(&mut Automerge) -> T,
    ) -> Option<T> {
        let mut state = self.shared.state.lock().await;
        state.projects.get_mut(project_id).map(|p| f(&mut p.doc))
    }

    /// A snapshot (fork) of the local document. `None` if unknown.
    pub async fn doc(&self, project_id: &str) -> Option<Automerge> {
        let state = self.shared.state.lock().await;
        state.projects.get(project_id).map(|p| p.doc.fork())
    }

    /// Dials the project's host and runs one full session: keyhive preamble,
    /// beelay handshake, bidirectional sedimentree sync (local commits
    /// upload, remote commits download), then decrypt-and-apply. Local edits
    /// are flushed (encrypted) before dialing so the preamble carries any
    /// CGKA ops the encryption produced. A host that closes the connection
    /// with the refusal code (this peer holds no role on the requested
    /// project, e.g. revoked) surfaces as [`JoinError::Refused`].
    pub async fn sync_project(
        &self,
        project_id: &str,
    ) -> std::result::Result<SyncOutcome, JoinError> {
        let (beelay_doc, host, unhealthy) = {
            let state = self.shared.state.lock().await;
            let proj = state
                .projects
                .get(project_id)
                .ok_or_else(|| anyerr!("unknown project {project_id}"))?;
            let host = match &proj.host {
                ProjectHost::Member(addr) => addr.clone(),
                ProjectHost::Hosted => {
                    return Err(anyerr!(
                        "project {project_id} has no host address; accept an invite first"
                    )
                    .into());
                }
                // NOT host-mode: a corrupt stored ticket must surface as its
                // own recoverable failure, not silent hosting behavior.
                ProjectHost::MemberBadTicket => {
                    return Err(JoinError::Other(AnyError::from_std(
                        HostTicketError::Unreadable,
                    )));
                }
            };
            (proj.beelay_doc, host, proj.unhealthy)
        };
        // an unhealthy doc is download-only; the refresh below can clear it.
        if !unhealthy {
            self.shared.flush(project_id).await?;
        }

        let conn = self
            .router
            .endpoint()
            .connect(host, BEELAY_ALPN)
            .await
            .context("dialing beelay host")?;
        let session = async {
            let (mut send, mut recv) = conn.open_bi().await.std_context("opening beelay stream")?;
            send_frame(&mut send, project_id.as_bytes()).await?;
            let peer = keyhive_preamble(
                &self.shared.auth,
                &self.shared.binding,
                conn.remote_id(),
                &mut send,
                &mut recv,
                true,
            )
            .await?;
            // remember the host's member id so its blob fetches pass the gate.
            self.shared
                .peers
                .lock()
                .unwrap()
                .insert(conn.remote_id(), peer);
            if let Err(e) = self
                .shared
                .persist_registry(self.shared.state.lock().await)
                .await
            {
                eprintln!("beelay: persisting peer map: {e}");
            }
            let connected = handshake_connect(
                self.shared.peer_id.clone(),
                conn.remote_id(),
                &mut send,
                &mut recv,
            )
            .await?;

            // pump the sync story to completion; the lock is taken per event.
            // The round cap bounds a host that keeps answering without ever
            // letting the story converge.
            let (story, event) = Event::sync_doc(beelay_doc, connected.their_peer_id().clone());
            let mut msgs = self
                .shared
                .state
                .lock()
                .await
                .core
                .drive_scoped(event, Some(beelay_doc))?;
            let mut rounds = 0usize;
            loop {
                send_envelopes(&connected, msgs, &mut send).await?;
                if let Some(result) = self.shared.state.lock().await.core.stories.remove(&story) {
                    let StoryResult::SyncDoc(_) = result else {
                        return Err(anyerr!("unexpected story result for sync_doc"));
                    };
                    break;
                }
                rounds += 1;
                if rounds > MAX_SYNC_ROUNDS {
                    return Err(anyerr!(
                        "beelay sync did not converge within {MAX_SYNC_ROUNDS} rounds"
                    ));
                }
                let frame = expect_tagged(&mut recv, TAG_BEELAY).await?;
                let msg = Message::decode(&frame).std_context("decoding beelay message")?;
                let env = connected
                    .receive(msg)
                    .std_context("routing beelay message")?;
                msgs = self
                    .shared
                    .state
                    .lock()
                    .await
                    .core
                    .drive_scoped(Event::receive(env), Some(beelay_doc))?;
            }

            // end-of-session: send bye, await the host's ack (sent after it
            // has applied our uploads — makes post-sync assertions
            // deterministic).
            send_frame(&mut send, &[]).await?;
            if recv_tagged(&mut recv).await?.is_some() {
                return Err(anyerr!("expected end-of-session ack"));
            }
            Ok::<_, AnyError>(())
        };
        // vendor-edit: a REFUSED_CODE close from the host means this peer is
        // denied; surface it typed, mirroring plain join().
        if let Err(e) = session.await {
            return match conn.close_reason() {
                Some(ConnectionError::ApplicationClosed(c))
                    if c.error_code == REFUSED_CODE.into() =>
                {
                    Err(JoinError::Refused)
                }
                _ => Err(JoinError::Other(e)),
            };
        }
        let outcome = self.shared.refresh(project_id).await?;
        conn.close(0u32.into(), b"done");
        Ok(outcome)
    }

    /// Encrypts `bytes` under the project key and serves the ciphertext as an
    /// iroh blob. Returns a pasteable ticket for [`Self::fetch_blob`] /
    /// [`Self::read_blob`]. Store AFTER granting members (same
    /// grant-before-encrypt ordering as project content, keyhive #136) and
    /// hand out invites after storing so their events cover this epoch.
    pub async fn store_blob(&self, project_id: &str, bytes: &[u8]) -> Result<String> {
        let sealed = self.shared.auth.encrypt(project_id, bytes).await?;
        let tag = self
            .blobs
            .add_bytes(sealed)
            .await
            .map_err(|e| anyerr!("adding blob: {e}"))?;
        // vendor-edit: claim the blob for this project in the blob -> doc
        // map so the blobs ALPN gate scopes fetches per project (spec §4).
        {
            let mut guard = self.shared.state.lock().await;
            let state = &mut *guard;
            let doc = state
                .projects
                .get(project_id)
                .ok_or_else(|| anyerr!("unknown project {project_id}"))?
                .beelay_doc;
            state.core.blob_docs.insert(tag.hash.to_hex(), doc);
            state.core.persist_blob_docs()?;
        }
        Ok(BlobTicket::new(self.addr(), tag.hash, BlobFormat::Raw).to_string())
    }

    /// Downloads the (still-encrypted) blob behind `ticket` into the local
    /// store, verified against its hash; the transfer is aborted once more
    /// than `max_bytes` of payload has arrived. Transfer only — decrypt by
    /// calling [`Self::read_blob`], so fetching can happen before/without
    /// access.
    ///
    /// Announces no project (empty announce frame), so the provider gates
    /// the requested hash against this member's role on the blob's own
    /// project. Prefer [`Self::fetch_blob_scoped`] when the project is known.
    pub async fn fetch_blob(&self, ticket: &str, max_bytes: u64) -> Result<()> {
        self.fetch_blob_inner(None, ticket, max_bytes).await
    }

    /// [`Self::fetch_blob`] with the dial announced for `project_id`
    /// (spec §4): the provider serves the blob only if it belongs to that
    /// project and this member holds Read or better on it.
    pub async fn fetch_blob_scoped(
        &self,
        project_id: &str,
        ticket: &str,
        max_bytes: u64,
    ) -> Result<()> {
        self.fetch_blob_inner(Some(project_id), ticket, max_bytes).await
    }

    async fn fetch_blob_inner(
        &self,
        project_id: Option<&str>,
        ticket: &str,
        max_bytes: u64,
    ) -> Result<()> {
        use futures::StreamExt as _;
        let ticket: BlobTicket = ticket
            .parse()
            .map_err(|e| anyerr!("bad blob ticket: {e}"))?;
        let conn = self
            .router
            .endpoint()
            .connect(ticket.addr().clone(), iroh_blobs::ALPN)
            .await
            .context("dialing blob provider")?;
        // vendor-edit: first uni stream announces the project the dial is
        // for, mirroring BeelayProtocol's project-id opener; an empty frame
        // means unscoped (the provider then gates per requested hash).
        let mut announce = conn
            .open_uni()
            .await
            .std_context("opening blob announce stream")?;
        send_frame(
            &mut announce,
            project_id.map(str::as_bytes).unwrap_or_default(),
        )
        .await?;
        announce.finish().std_context("closing blob announce stream")?;
        let mut progress = std::pin::pin!(self.blobs.remote().fetch(conn, ticket.hash()).stream());
        while let Some(item) = progress.next().await {
            match item {
                // dropping the stream cancels the in-flight transfer.
                GetProgressItem::Progress(n) if n > max_bytes => {
                    return Err(AnyError::from_std(BlobError::TooLarge));
                }
                GetProgressItem::Progress(_) => {}
                GetProgressItem::Done(_) => return Ok(()),
                GetProgressItem::Error(e) => return Err(anyerr!("fetching blob: {e}")),
            }
        }
        Err(anyerr!("blob fetch ended without completing"))
    }

    /// Whether `ticket`'s blob is already in the local store, without
    /// attempting to decrypt it — lets a caller tell "not fetched yet" apart
    /// from a decrypt failure.
    pub async fn has_blob(&self, ticket: &str) -> bool {
        let Ok(ticket) = ticket.parse::<BlobTicket>() else {
            return false;
        };
        self.blobs.has(ticket.hash()).await.unwrap_or(false)
    }

    /// Decrypts a locally-stored blob ([`Self::store_blob`]d here or
    /// [`Self::fetch_blob`]ed) back to the original bytes; errors if the
    /// stored ciphertext exceeds `max_bytes`.
    pub async fn read_blob(
        &self,
        project_id: &str,
        ticket: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        let ticket: BlobTicket = ticket
            .parse()
            .map_err(|e| anyerr!("bad blob ticket: {e}"))?;
        let sealed = self
            .blobs
            .get_bytes(ticket.hash())
            .await
            .map_err(|e| anyerr!("blob not in local store (fetch it first?): {e}"))?;
        if sealed.len() as u64 > max_bytes {
            return Err(AnyError::from_std(BlobError::TooLarge));
        }
        self.shared
            .auth
            .decrypt(project_id, &sealed)
            .await
            .map_err(|e| AnyError::from_std(BlobError::Decrypt(e)))
    }

    /// Graceful shutdown: stops accepting and closes the endpoint, then
    /// waits for the kv writer's final flush. The router also shuts the
    /// blob store down (its `BlobsProtocol` handler).
    pub async fn shutdown(&self) -> Result<()> {
        let result = self.router.shutdown().await.std_context("router shutdown");
        let (kv, blob_kv) = {
            let mut state = self.shared.state.lock().await;
            (state.core.kv.take(), state.core.blob_kv.take())
        };
        for kv in [kv, blob_kv].into_iter().flatten() {
            drop(kv.tx);
            let _ = kv.task.await;
        }
        result
    }
}

// --- single-endpoint stack ----------------------------------------------------------

/// Binds the whole share stack on ONE iroh endpoint and router — plain sync
/// ([`crate::ALPN`]), beelay ([`BEELAY_ALPN`]), and blobs — so the device
/// identity announces a single address instead of two endpoints flapping.
/// Router shutdown is shared between [`ShareNode::shutdown`] and
/// [`BeelayNode::shutdown`].
pub async fn bind_stack(
    identity: &DeviceIdentity,
    auth_identity: &AuthIdentity,
    auth: ProjectAuth,
    data_dir: Option<&Path>,
) -> Result<(ShareNode, BeelayNode)> {
    bind_stack_with(identity, auth_identity, auth, data_dir, presets::N0).await
}

/// [`bind_stack`] without discovery or relays: peers must dial full addresses.
/// Offline/LAN use and tests.
pub async fn bind_stack_local(
    identity: &DeviceIdentity,
    auth_identity: &AuthIdentity,
    auth: ProjectAuth,
    data_dir: Option<&Path>,
) -> Result<(ShareNode, BeelayNode)> {
    bind_stack_with(identity, auth_identity, auth, data_dir, presets::Minimal).await
}

async fn bind_stack_with(
    identity: &DeviceIdentity,
    auth_identity: &AuthIdentity,
    auth: ProjectAuth,
    data_dir: Option<&Path>,
    preset: impl presets::Preset,
) -> Result<(ShareNode, BeelayNode)> {
    let endpoint = Endpoint::builder(preset)
        .secret_key(identity.secret().clone())
        .bind()
        .await
        .context("binding iroh endpoint")?;
    let binding = DeviceBinding::create(identity, auth_identity);
    let (shared, blobs) = BeelayNode::prepare(endpoint.id(), auth, binding, data_dir).await?;
    let (sync_proto, projects, access_check) = ShareNode::parts();
    let router = Router::builder(endpoint)
        .accept(crate::sync::ALPN, sync_proto)
        .accept(
            BEELAY_ALPN,
            BeelayProtocol {
                shared: shared.clone(),
            },
        )
        .accept(
            iroh_blobs::ALPN,
            GatedBlobs {
                inner: BlobsProtocol::new(&blobs, None),
                shared: shared.clone(),
            },
        )
        .spawn();
    let share = ShareNode::from_parts(router.clone(), projects, access_check);
    let beelay = BeelayNode::finish(router, shared, blobs).await?;
    Ok((share, beelay))
}

// --- protocol handler --------------------------------------------------------------

#[derive(Clone)]
struct BeelayProtocol {
    shared: Arc<Shared>,
}

impl fmt::Debug for BeelayProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BeelayProtocol")
    }
}

/// Blobs handler scoped per project (write-enforcement spec §4): the dialer
/// announces a project id on its first uni stream (empty frame = unscoped
/// dial), and every requested hash must belong — per the blob -> doc map —
/// to a project the session-learned member may read. An announced dial pins
/// the whole connection to that one project; an unscoped dial is gated per
/// request against the requested blob's own project. Fetch needs Read or
/// better; Relay holds no content key and is refused.
///
/// Write side needs no gate here: the provider runs with iroh-blobs'
/// default `push: RequestMode::Disabled`, which rejects every remote
/// `Request::Push` with a permission error before it touches the store —
/// the handler is fetch-only, so no role (Read included) can store or
/// replace blobs over this ALPN.
struct GatedBlobs {
    inner: BlobsProtocol,
    shared: Arc<Shared>,
}

impl fmt::Debug for GatedBlobs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GatedBlobs")
    }
}

/// Whether `member` may fetch blobs of `project_id`: any role but Relay.
async fn blob_role_allows(shared: &Shared, project_id: &str, member: MemberId) -> bool {
    matches!(
        shared.auth.query_access(project_id, member).await,
        Ok(Some(role)) if role != Role::Relay
    )
}

/// Per-request gate: `hash` must be mapped, and to the announced doc when
/// the dial announced one, else to a project `member` may read.
async fn blob_allowed(
    shared: &Shared,
    member: MemberId,
    announced: Option<DocumentId>,
    hash: &iroh_blobs::Hash,
) -> bool {
    let (doc, project) = {
        let state = shared.state.lock().await;
        // vendor-edit: an unmapped blob (stored before the map existed) is
        // DENIED — the flagged layer has no real deployments, so there are
        // no legacy blobs to migrate.
        let Some(doc) = state.core.blob_docs.get(&hash.to_hex()).copied() else {
            return false;
        };
        let project = state
            .projects
            .iter()
            .find(|(_, p)| p.beelay_doc == doc)
            .map(|(id, _)| id.clone());
        (doc, project)
    };
    match announced {
        Some(announced) => announced == doc,
        None => match project {
            Some(project) => blob_role_allows(shared, &project, member).await,
            None => false,
        },
    }
}

impl ProtocolHandler for GatedBlobs {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let remote = conn.remote_id();
        // the dialer's first uni stream announces the project (empty frame =
        // unscoped dial); deadline-bounded like every pre-auth read.
        let mut announce = tokio::time::timeout(RECV_TIMEOUT, conn.accept_uni())
            .await
            .map_err(|_| anyerr!("no blob announce stream within deadline"))??;
        let announced = recv_frame_max(&mut announce, MAX_PROJECT_ID_FRAME as u64).await?;
        let member = self.shared.peers.lock().unwrap().get(&remote).copied();
        let Some(member) = member else {
            conn.close(REFUSED_CODE.into(), b"refused");
            return Ok(());
        };
        let announced_doc = match &announced {
            Some(project) => {
                let project =
                    std::str::from_utf8(project).map_err(AcceptError::from_err)?;
                let doc = self
                    .shared
                    .state
                    .lock()
                    .await
                    .projects
                    .get(project)
                    .map(|p| p.beelay_doc);
                match doc {
                    Some(doc) if blob_role_allows(&self.shared, project, member).await => {
                        Some(doc)
                    }
                    _ => {
                        conn.close(REFUSED_CODE.into(), b"refused");
                        return Ok(());
                    }
                }
            }
            None => None,
        };
        // per-request gate: intercept events answer allow/deny for each
        // requested hash; push stays Disabled (see the type docs), observe
        // is denied outright (no client of ours sends it).
        let (events, mut rx) = EventSender::channel(
            16,
            EventMask {
                get: RequestMode::Intercept,
                get_many: RequestMode::Intercept,
                observe: ObserveMode::Intercept,
                ..EventMask::DEFAULT
            },
        );
        let shared = self.shared.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    ProviderMessage::GetRequestReceived(m) => {
                        let ok =
                            blob_allowed(&shared, member, announced_doc, &m.inner.request.hash)
                                .await;
                        let verdict = ok.then_some(()).ok_or(AbortReason::Permission);
                        m.tx.send(verdict).await.ok();
                    }
                    ProviderMessage::GetManyRequestReceived(m) => {
                        let mut ok = true;
                        for hash in &m.inner.request.hashes {
                            if !blob_allowed(&shared, member, announced_doc, hash).await {
                                ok = false;
                                break;
                            }
                        }
                        let verdict = ok.then_some(()).ok_or(AbortReason::Permission);
                        m.tx.send(verdict).await.ok();
                    }
                    ProviderMessage::ObserveRequestReceived(m) => {
                        m.tx.send(Err(AbortReason::Permission)).await.ok();
                    }
                    // only the intercepts above are enabled in the mask
                    _ => {}
                }
            }
        });
        handle_connection(conn, self.inner.store().clone(), events).await;
        Ok(())
    }

    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}

impl ProtocolHandler for BeelayProtocol {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let remote = conn.remote_id();
        let (mut send, mut recv) = conn.accept_bi().await?;
        let project_id = recv_frame_max(&mut recv, MAX_PROJECT_ID_FRAME as u64)
            .await?
            .ok_or_else(|| anyerr!("initiator sent empty project id"))?;
        let project_id = String::from_utf8(project_id).map_err(AcceptError::from_err)?;
        let (scope, unhealthy) = {
            let state = self.shared.state.lock().await;
            match state.projects.get(&project_id) {
                Some(p) => (Some(p.beelay_doc), p.unhealthy),
                None => (None, false),
            }
        };
        let Some(scope) = scope else {
            conn.close(REFUSED_CODE.into(), b"refused");
            return Ok(());
        };
        // Encrypt pending local edits BEFORE the preamble so the events we
        // export include any CGKA ops the encryption produced. An unhealthy
        // doc is not flushed; the refresh at session end can clear it.
        if !unhealthy {
            self.shared.flush(&project_id).await?;
        }
        let peer = keyhive_preamble(
            &self.shared.auth,
            &self.shared.binding,
            remote,
            &mut send,
            &mut recv,
            false,
        )
        .await?;
        // vendor-edit: membership gate scoped to the requested project only,
        // branched on the peer's role (write-enforcement spec §2.3):
        // Admin/Edit sync bidirectionally against the real core; Read is
        // served from a session-local scratch core so anything it uploads
        // evaporates (§2.4 mechanism B); Relay is reserved — no content key
        // to serve under the dumb-transport model — and refused like a
        // non-member until a ciphertext-forward path exists.
        let mut scratch = match self.shared.auth.query_access(&project_id, peer).await? {
            Some(Role::Admin | Role::Edit) => None,
            Some(Role::Read) => {
                // snapshot AFTER the flush above so the clone carries the
                // newest commits. Whole-map clone: load_doc_commits unwraps
                // blob loads, so sedimentree keys must bring their blobs.
                let state = self.shared.state.lock().await;
                Some(Core::scratch(
                    self.shared.peer_id.clone(),
                    state.core.storage.clone(),
                    state.core.blob_docs.clone(),
                ))
            }
            Some(Role::Relay) | None => {
                conn.close(REFUSED_CODE.into(), b"refused");
                return Ok(());
            }
        };
        self.shared.peers.lock().unwrap().insert(remote, peer);
        if let Err(e) = self
            .shared
            .persist_registry(self.shared.state.lock().await)
            .await
        {
            eprintln!("beelay: persisting peer map: {e}");
        }
        let connected =
            handshake_accept(self.shared.peer_id.clone(), remote, &mut send, &mut recv).await?;

        // purely reactive: the initiator drives; we answer until its bye. The
        // frame cap bounds an initiator that never says bye but keeps the
        // session alive with traffic (which no read timeout catches).
        let mut got_bye = false;
        for _ in 0..MAX_SYNC_ROUNDS {
            match recv_tagged(&mut recv).await? {
                None => {
                    got_bye = true;
                    break;
                }
                Some((TAG_BEELAY, frame)) => {
                    // vendor-edit: beelay-core =0.1.0-alpha.1 answers an
                    // incoming Request::UploadBlob with todo!() — a panic
                    // any dialer could trigger inside handle_event. The
                    // encoding has no varints before the request-type byte,
                    // so the offsets are fixed: frame[0]=2 (stream Data),
                    // frame[1]=0 (Request), frame[2..18] RequestId,
                    // frame[18]=3 (RequestType::UploadBlob). No client in
                    // the crate ever sends it; reject for every role before
                    // the frame reaches either core.
                    if frame.len() > 18 && frame[0] == 2 && frame[1] == 0 && frame[18] == 3 {
                        return Err(AcceptError::from(anyerr!(
                            "peer sent an UploadBlob request (unsupported)"
                        )));
                    }
                    let msg = Message::decode(&frame).map_err(AcceptError::from_err)?;
                    let env = connected.receive(msg).map_err(AcceptError::from_err)?;
                    // scoped: this session may only touch the announced doc.
                    // Read sessions run against the session-owned scratch
                    // core — no shared lock, uploads die with it.
                    let msgs = match scratch.as_mut() {
                        Some(core) => core.drive_scoped(Event::receive(env), Some(scope))?,
                        None => self
                            .shared
                            .state
                            .lock()
                            .await
                            .core
                            .drive_scoped(Event::receive(env), Some(scope))?,
                    };
                    send_envelopes(&connected, msgs, &mut send).await?;
                }
                Some((tag, _)) => {
                    return Err(AcceptError::from(anyerr!(
                        "unexpected frame tag {tag} mid-session"
                    )));
                }
            }
        }
        if !got_bye {
            return Err(AcceptError::from(anyerr!(
                "beelay session exceeded {MAX_SYNC_ROUNDS} frames without ending"
            )));
        }
        // apply whatever the initiator uploaded, then ack its bye. A Read
        // session wrote nothing to the real core — skip the refresh, but
        // still ack so the initiator's protocol expectation holds.
        if scratch.is_none() {
            self.shared.refresh(&project_id).await?;
        }
        send_frame(&mut send, &[]).await?;
        // wait for the initiator to close so tail data isn't dropped.
        conn.closed().await;
        Ok(())
    }
}

// --- tests -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use automerge::{ROOT, transaction::Transactable};

    /// The flush-side map (change -> commit, parents = mapped deps) must be
    /// exactly rebuildable on the receive side from the commits alone, even
    /// when they arrive in reverse order.
    #[test]
    fn change_commit_mapping_roundtrip() {
        let mut doc = Automerge::new();
        doc.transact(|tx| tx.put(ROOT, "a", 1)).unwrap();
        doc.transact(|tx| tx.put(ROOT, "b", 2)).unwrap();
        // concurrent branch so the DAG has a merge
        let mut branch = doc.fork();
        branch.transact(|tx| tx.put(ROOT, "c", 3)).unwrap();
        doc.transact(|tx| tx.put(ROOT, "d", 4)).unwrap();
        doc.merge(&mut branch).unwrap();

        // sender side, with passthrough "encryption"
        let mut map: HashMap<ChangeHash, CommitHash> = HashMap::new();
        let changes = unflushed_changes(&doc, &map).unwrap();
        assert_eq!(changes.len(), 4);
        let mut commits = Vec::new();
        for change in &changes {
            let parents = commit_parents(change, &map).unwrap();
            let contents = change.raw_bytes().to_vec();
            let hash = CommitHash::from(*blake3::hash(&contents).as_bytes());
            map.insert(change.hash(), hash);
            commits.push(Commit::new(parents, contents, hash));
        }

        // receive side: rebuild map + doc from commits alone, worst-case order
        commits.reverse();
        let mut rebuilt_map = HashMap::new();
        let mut rebuilt_changes = Vec::new();
        for commit in &commits {
            let change = Change::from_bytes(commit.contents().to_vec()).unwrap();
            // parents must be the mapped deps of the decoded change
            let expected: Vec<CommitHash> = change
                .deps()
                .iter()
                .map(|d| {
                    let c = commits
                        .iter()
                        .find(|c| Change::from_bytes(c.contents().to_vec()).unwrap().hash() == *d)
                        .unwrap();
                    c.hash()
                })
                .collect();
            assert_eq!(commit.parents(), expected);
            rebuilt_map.insert(change.hash(), commit.hash());
            rebuilt_changes.push(change);
        }
        let mut rebuilt = Automerge::new();
        rebuilt.apply_changes(rebuilt_changes).unwrap();

        assert_eq!(rebuilt_map, map);
        assert_eq!(rebuilt.get_heads(), doc.get_heads());
        // the receiver's next flush sees nothing new -> same hashes both sides
        assert!(
            unflushed_changes(&rebuilt, &rebuilt_map)
                .unwrap()
                .is_empty()
        );
    }

    /// Deps always precede dependents regardless of get_changes order.
    #[test]
    fn unflushed_changes_orders_deps_first() {
        let mut doc = Automerge::new();
        for i in 0..5 {
            doc.transact(|tx| tx.put(ROOT, "k", i)).unwrap();
        }
        let ordered = unflushed_changes(&doc, &HashMap::new()).unwrap();
        let mut seen = HashSet::new();
        for change in &ordered {
            for dep in change.deps() {
                assert!(seen.contains(dep), "dep emitted after dependent");
            }
            seen.insert(change.hash());
        }
        assert_eq!(ordered.len(), 5);
    }
}

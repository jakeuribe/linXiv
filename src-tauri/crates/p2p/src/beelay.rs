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
    str::FromStr,
    sync::Arc,
};

use automerge::{Automerge, Change, ChangeHash};
use beelay_core::{
    Beelay, Commit, CommitHash, CommitOrBundle, DocumentId, Envelope, Event, PeerId, StorageKey,
    StoryId, StoryResult,
    io::{IoAction, IoResult, IoTask},
    messages::stream::{Connected, Connecting, Message, Step},
};
use iroh::{
    Endpoint, EndpointAddr, EndpointId,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{BlobFormat, BlobsProtocol, store::mem::MemStore, ticket::BlobTicket};
use iroh_tickets::{ParseError, Ticket, endpoint::EndpointTicket};
use n0_error::{Result, StackResultExt, StdResultExt, anyerr};
use rand::rngs::OsRng;
use tokio::sync::Mutex;

use crate::{
    auth::{DecryptError, MemberId, ProjectAuth},
    sync::{DeviceIdentity, MAX_SYNC_ROUNDS, recv_frame, send_frame},
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
            &self.events,
        ))
        .expect("postcard serialization failed")
    }

    fn decode_bytes(bytes: &[u8]) -> std::result::Result<Self, ParseError> {
        let (endpoint, project_id, beelay_doc, keyhive_doc, events) = postcard::from_bytes(bytes)?;
        Ok(Self {
            endpoint,
            project_id,
            beelay_doc,
            keyhive_doc,
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

/// The sans-IO beelay state machine plus its synchronously-answered storage.
struct Core {
    beelay: Beelay<OsRng>,
    // ponytail: in-memory KV — every restart re-syncs from peers. Swap for a
    // disk-backed store answering IoTasks when persistence matters.
    storage: BTreeMap<StorageKey, Vec<u8>>,
    stories: HashMap<StoryId, StoryResult>,
}

impl Core {
    fn new(peer_id: PeerId) -> Self {
        Self {
            beelay: Beelay::new(peer_id, OsRng),
            storage: BTreeMap::new(),
            stories: HashMap::new(),
        }
    }

    /// Feeds `event` through the state machine, answering every IoTask from
    /// the KV until quiescent. Returns outbound envelopes; completed stories
    /// land in `self.stories`.
    fn drive(&mut self, event: Event) -> Result<Vec<Envelope>> {
        let mut inbox = VecDeque::from([event]);
        let mut out = Vec::new();
        while let Some(event) = inbox.pop_front() {
            let results = self
                .beelay
                .handle_event(event)
                .map_err(|e| anyerr!("beelay: {e}"))?;
            out.extend(results.new_messages);
            for task in results.new_tasks {
                inbox.push_back(Event::io_complete(self.io(task)));
            }
            self.stories.extend(results.completed_stories);
            // notifications only come from `listen` stories, which we never
            // start — sessions are one-shot pulls like Phase 1.
        }
        Ok(out)
    }

    fn io(&mut self, task: IoTask) -> IoResult {
        let id = task.id();
        match task.take_action() {
            IoAction::Load { key } => IoResult::load(id, self.storage.get(&key).cloned()),
            IoAction::LoadRange { prefix } => IoResult::load_range(
                id,
                self.storage
                    .iter()
                    .filter(|(k, _)| prefix.is_prefix_of(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            IoAction::Put { key, data } => {
                self.storage.insert(key, data);
                IoResult::put(id)
            }
            IoAction::Delete { key } => {
                self.storage.remove(&key);
                IoResult::delete(id)
            }
            // pure p2p: never forward requests for a doc to other peers.
            IoAction::Ask { .. } => IoResult::ask(id, HashSet::new()),
        }
    }

    /// Result of a story that completes without network round trips.
    fn take_story(&mut self, id: StoryId) -> Result<StoryResult> {
        self.stories
            .remove(&id)
            .ok_or_else(|| anyerr!("beelay story did not complete synchronously"))
    }
}

// --- node state ------------------------------------------------------------------

struct ProjectState {
    /// The local plaintext document.
    doc: Automerge,
    beelay_doc: DocumentId,
    /// Where [`BeelayNode::sync_project`] dials; set by `accept_invite`.
    host: Option<EndpointAddr>,
    /// automerge change hash -> beelay commit hash (commit = sealed change).
    change_to_commit: HashMap<ChangeHash, CommitHash>,
    /// Commits already applied to `doc` (or produced from it).
    applied: HashSet<CommitHash>,
}

struct State {
    core: Core,
    projects: HashMap<String, ProjectState>,
}

struct Shared {
    auth: ProjectAuth,
    peer_id: PeerId,
    // Beelay is Send-not-Sync; the tokio Mutex makes it shareable. The lock is
    // never held across a network await — only across local compute (including
    // keyhive encrypt/decrypt).
    state: Mutex<State>,
}

impl Shared {
    async fn project_ids(&self) -> Vec<String> {
        self.state.lock().await.projects.keys().cloned().collect()
    }

    /// Encrypts every not-yet-flushed local change into a beelay commit.
    async fn flush(&self, project_id: &str) -> Result<()> {
        let mut guard = self.state.lock().await;
        let state = &mut *guard;
        let proj = state
            .projects
            .get_mut(project_id)
            .ok_or_else(|| anyerr!("unknown project {project_id}"))?;
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
        state.core.drive(event)?;
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

/// Both sides swap contact cards, then swap + ingest each other's keyhive
/// static events, so CGKA ops arrive before any beelay traffic. `initiate`
/// picks send-first vs receive-first (like `run_sync`): a symmetric
/// write-then-read on both ends deadlocks once the event payloads outgrow the
/// QUIC stream flow-control window, since each side's write waits for a read
/// the other side never starts.
async fn keyhive_preamble(
    auth: &ProjectAuth,
    send: &mut SendStream,
    recv: &mut RecvStream,
    initiate: bool,
) -> Result<MemberId> {
    let peer;
    let events;
    if initiate {
        send_tagged(send, TAG_KEYHIVE, &auth.contact_card().await?).await?;
        let card = expect_keyhive(recv).await?;
        peer = auth.receive_contact_card(&card).await?;
        send_tagged(send, TAG_KEYHIVE, &auth.export_events_for(peer).await?).await?;
        events = expect_keyhive(recv).await?;
    } else {
        let card = expect_keyhive(recv).await?;
        peer = auth.receive_contact_card(&card).await?;
        send_tagged(send, TAG_KEYHIVE, &auth.contact_card().await?).await?;
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
    // ponytail: in-memory blob store — blobs vanish on restart, the owner
    // re-stores from its PDF files on disk. Swap for FsStore when caching
    // fetched blobs across runs matters.
    blobs: MemStore,
}

impl fmt::Debug for BeelayNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BeelayNode({})", self.endpoint_id())
    }
}

impl BeelayNode {
    /// Binds with n0 discovery + relays: dialable by bare [`EndpointId`].
    pub async fn bind(identity: &DeviceIdentity, auth: ProjectAuth) -> Result<Self> {
        Self::bind_with(identity, auth, presets::N0).await
    }

    /// Binds without discovery or relays: peers must dial the full
    /// [`EndpointAddr`] carried in invites. Offline/LAN use and tests.
    pub async fn bind_local(identity: &DeviceIdentity, auth: ProjectAuth) -> Result<Self> {
        Self::bind_with(identity, auth, presets::Minimal).await
    }

    async fn bind_with(
        identity: &DeviceIdentity,
        auth: ProjectAuth,
        preset: impl presets::Preset,
    ) -> Result<Self> {
        let endpoint = Endpoint::builder(preset)
            .secret_key(identity.secret().clone())
            .bind()
            .await
            .context("binding iroh endpoint")?;
        let peer_id = PeerId::from(endpoint.id().to_string());
        let shared = Arc::new(Shared {
            auth,
            peer_id: peer_id.clone(),
            state: Mutex::new(State {
                core: Core::new(peer_id),
                projects: HashMap::new(),
            }),
        });
        let blobs = MemStore::new();
        let router = Router::builder(endpoint)
            .accept(
                BEELAY_ALPN,
                BeelayProtocol {
                    shared: shared.clone(),
                },
            )
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, None))
            .spawn();
        Ok(Self {
            router,
            shared,
            blobs,
        })
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
                host: None,
                change_to_commit: HashMap::new(),
                applied: HashSet::new(),
            },
        );
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
            events,
        };
        Ok(invite.encode_string())
    }

    /// Ingests an invite: learns the delegations, adopts the keyhive doc,
    /// and registers an empty local doc wired to the host's address. Returns
    /// the project id; call [`Self::sync_project`] to fetch the content.
    pub async fn accept_invite(&self, invite: &str) -> Result<String> {
        let invite: ProjectInvite = invite
            .parse()
            .map_err(|e| anyerr!("malformed invite: {e}"))?;
        // hold the lock across check + ingest + adopt + insert so concurrent
        // accepts of the same invite can't both pass the existence check
        // (ingest/adopt are local keyhive compute — no network awaits).
        let mut state = self.shared.state.lock().await;
        if state.projects.contains_key(&invite.project_id) {
            // never clobber local edits with a fresh empty doc
            return Err(anyerr!("project {} already exists", invite.project_id));
        }
        self.shared.auth.ingest_events(&invite.events).await?;
        self.shared
            .auth
            .adopt_project(&invite.project_id, invite.keyhive_doc)
            .await?;
        state.projects.insert(
            invite.project_id.clone(),
            ProjectState {
                doc: Automerge::new(),
                beelay_doc: DocumentId::from(invite.beelay_doc),
                host: Some(invite.endpoint.endpoint_addr().clone()),
                change_to_commit: HashMap::new(),
                applied: HashSet::new(),
            },
        );
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
    /// CGKA ops the encryption produced.
    pub async fn sync_project(&self, project_id: &str) -> Result<SyncOutcome> {
        let (beelay_doc, host) = {
            let state = self.shared.state.lock().await;
            let proj = state
                .projects
                .get(project_id)
                .ok_or_else(|| anyerr!("unknown project {project_id}"))?;
            let host = proj.host.clone().ok_or_else(|| {
                anyerr!("project {project_id} has no host address; accept an invite first")
            })?;
            (proj.beelay_doc, host)
        };
        self.shared.flush(project_id).await?;

        let conn = self
            .router
            .endpoint()
            .connect(host, BEELAY_ALPN)
            .await
            .context("dialing beelay host")?;
        let (mut send, mut recv) = conn.open_bi().await.std_context("opening beelay stream")?;
        keyhive_preamble(&self.shared.auth, &mut send, &mut recv, true).await?;
        let connected = handshake_connect(
            self.shared.peer_id.clone(),
            conn.remote_id(),
            &mut send,
            &mut recv,
        )
        .await?;

        // pump the sync story to completion; the lock is taken per event and
        // never held across a network await. The round cap bounds a host that
        // keeps answering without ever letting the story converge.
        let (story, event) = Event::sync_doc(beelay_doc, connected.their_peer_id().clone());
        let mut msgs = self.shared.state.lock().await.core.drive(event)?;
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
                .drive(Event::receive(env))?;
        }

        // end-of-session: send bye, await the host's ack (sent after it has
        // applied our uploads — makes post-sync assertions deterministic).
        send_frame(&mut send, &[]).await?;
        if recv_tagged(&mut recv).await?.is_some() {
            return Err(anyerr!("expected end-of-session ack"));
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
        Ok(BlobTicket::new(self.addr(), tag.hash, BlobFormat::Raw).to_string())
    }

    /// Downloads the (still-encrypted) blob behind `ticket` into the local
    /// store, verified against its hash. Transfer only — decrypt by calling
    /// [`Self::read_blob`], so fetching can happen before/without access.
    pub async fn fetch_blob(&self, ticket: &str) -> Result<()> {
        let ticket: BlobTicket = ticket
            .parse()
            .map_err(|e| anyerr!("bad blob ticket: {e}"))?;
        let conn = self
            .router
            .endpoint()
            .connect(ticket.addr().clone(), iroh_blobs::ALPN)
            .await
            .context("dialing blob provider")?;
        self.blobs
            .remote()
            .fetch(conn, ticket.hash())
            .await
            .map_err(|e| anyerr!("fetching blob: {e}"))?;
        Ok(())
    }

    /// Decrypts a locally-stored blob ([`Self::store_blob`]d here or
    /// [`Self::fetch_blob`]ed) back to the original bytes.
    pub async fn read_blob(&self, project_id: &str, ticket: &str) -> Result<Vec<u8>> {
        let ticket: BlobTicket = ticket
            .parse()
            .map_err(|e| anyerr!("bad blob ticket: {e}"))?;
        let sealed = self
            .blobs
            .get_bytes(ticket.hash())
            .await
            .map_err(|e| anyerr!("blob not in local store (fetch it first?): {e}"))?;
        self.shared
            .auth
            .decrypt(project_id, &sealed)
            .await
            .map_err(|e| anyerr!("decrypting blob: {e}"))
    }

    /// Graceful shutdown: stops accepting and closes the endpoint.
    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await.std_context("router shutdown")
    }
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

impl ProtocolHandler for BeelayProtocol {
    async fn accept(&self, conn: Connection) -> std::result::Result<(), AcceptError> {
        let remote = conn.remote_id();
        let (mut send, mut recv) = conn.accept_bi().await?;
        // Encrypt pending local edits BEFORE the preamble so the events we
        // export include any CGKA ops the encryption produced.
        // ponytail: flushes every registered project — which doc the peer
        // wants is only known inside beelay; go per-doc when scale demands.
        for id in self.shared.project_ids().await {
            self.shared.flush(&id).await?;
        }
        keyhive_preamble(&self.shared.auth, &mut send, &mut recv, false).await?;
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
                    let msg = Message::decode(&frame).map_err(AcceptError::from_err)?;
                    let env = connected.receive(msg).map_err(AcceptError::from_err)?;
                    let msgs = self
                        .shared
                        .state
                        .lock()
                        .await
                        .core
                        .drive(Event::receive(env))?;
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
        // apply whatever the initiator uploaded, then ack its bye.
        for id in self.shared.project_ids().await {
            self.shared.refresh(&id).await?;
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

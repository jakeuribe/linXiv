//! p2p-sync: iroh transport, share sessions, automerge sync (Phase 1),
//! beelay-over-iroh behind `sync-beelay` (Phase 3).

use std::{
    collections::HashMap,
    fmt,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use automerge::{
    Automerge,
    sync::{self, SyncDoc},
};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_tickets::{ParseError, Ticket, endpoint::EndpointTicket};
use n0_error::{AnyError, Result, StackResultExt, StdResultExt, anyerr};

/// ALPN for linXiv project-sync sessions.
pub const ALPN: &[u8] = b"linxiv/sync/0";

// vendor-edit: connection close code the host sends for an unknown/denied
// project, so join() can tell a refusal from a transport failure.
const REFUSED_CODE: u32 = 1;

/// Largest accepted sync frame.
// ponytail: 64 MiB cap on untrusted frame lengths; raise if project docs outgrow it.
const MAX_FRAME: u64 = 64 * 1024 * 1024;

/// Deadline for one complete frame to arrive. Bounds both the silent peer and
/// the slow-trickle peer — without it an attacker's declared-but-never-sent
/// frame pins its buffer (up to [`MAX_FRAME`]) and the handler task forever.
const RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Cap on lockstep rounds per sync session; convergence takes a handful, so a
/// peer still exchanging traffic after this many rounds is never converging.
pub(crate) const MAX_SYNC_ROUNDS: usize = 10_000;

// --- device identity -------------------------------------------------------

/// Persistent device identity: an iroh secret key stored on disk.
///
/// The [`EndpointId`] derived from it is the device's share identity.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    secret: SecretKey,
}

impl DeviceIdentity {
    /// Loads the key at `path`, generating and persisting a new one if the file
    /// doesn't exist yet. Same path always yields the same [`EndpointId`].
    pub fn load_or_generate(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let secret = if path.exists() {
            let bytes = std::fs::read(path)?;
            let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("device key file {} is not 32 bytes", path.display()),
                )
            })?;
            SecretKey::from_bytes(&bytes)
        } else {
            let key = SecretKey::generate();
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            write_key(path, &key.to_bytes())?;
            key
        };
        Ok(Self { secret })
    }

    /// This device's share identity.
    pub fn endpoint_id(&self) -> EndpointId {
        self.secret.public()
    }

    /// The raw secret, for crate-internal signing (device binding).
    #[cfg(feature = "auth-keyhive")]
    pub(crate) fn secret(&self) -> &SecretKey {
        &self.secret
    }
}

#[cfg(unix)]
pub(crate) fn write_key(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    // private key: owner-only, and create_new so a concurrent generator errors
    // instead of silently clobbering.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
pub(crate) fn write_key(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

// --- access check ------------------------------------------------------------

/// Callback consulted before serving a project sync: `(peer, project_id) ->
/// allowed?`. Deliberately keyhive-free so this module never depends on the
/// capability layer; the `auth` module builds one from its membership state.
pub type AccessCheckFn = Arc<dyn Fn(EndpointId, &str) -> bool + Send + Sync>;

#[derive(Clone, Default)]
struct AccessCheck(Arc<Mutex<Option<AccessCheckFn>>>);

impl AccessCheck {
    fn allows(&self, peer: EndpointId, project_id: &str) -> bool {
        match &*self.0.lock().unwrap() {
            Some(check) => check(peer, project_id),
            None => true,
        }
    }
}

impl fmt::Debug for AccessCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let set = self.0.lock().unwrap().is_some();
        write!(f, "AccessCheck {{ set: {set} }}")
    }
}

// --- share ticket ----------------------------------------------------------

/// A pasteable invite: the host's [`EndpointAddr`] plus the project id.
///
/// Round-trips through its `Display`/`FromStr` string form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareTicket {
    endpoint: EndpointTicket,
    project_id: String,
}

impl ShareTicket {
    /// Ticket for `project_id` hosted at `addr`. Accepts a full [`EndpointAddr`]
    /// or a bare [`EndpointId`]; the latter needs discovery enabled on both sides.
    pub fn new(addr: impl Into<EndpointAddr>, project_id: impl Into<String>) -> Self {
        Self {
            endpoint: EndpointTicket::new(addr.into()),
            project_id: project_id.into(),
        }
    }

    /// The shared project's id.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// The hosting device's share identity.
    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.endpoint_addr().id
    }

    // vendor-edit: expose the full dialable address (endpoint_id() drops the
    // direct addrs, which relay-less/bind_local dialing needs).
    /// The hosting device's dialable address.
    pub fn endpoint_addr(&self) -> &EndpointAddr {
        self.endpoint.endpoint_addr()
    }
}

impl Ticket for ShareTicket {
    const KIND: &'static str = "linxivshare";

    fn encode_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(&self.endpoint, &self.project_id))
            .expect("postcard serialization failed")
    }

    fn decode_bytes(bytes: &[u8]) -> Result<Self, ParseError> {
        let (endpoint, project_id) = postcard::from_bytes(bytes)?;
        Ok(Self {
            endpoint,
            project_id,
        })
    }
}

impl fmt::Display for ShareTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode_string())
    }
}

impl FromStr for ShareTicket {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        Self::decode_string(s)
    }
}

// --- share node ------------------------------------------------------------

type Projects = Arc<Mutex<HashMap<String, Automerge>>>;

/// A bound p2p node holding the registry of shared project documents.
#[derive(Debug)]
pub struct ShareNode {
    router: Router,
    projects: Projects,
    // vendor-edit: access_check ungated — the linXiv share layer installs a
    // keyhive-free filesystem check.
    access_check: AccessCheck,
}

impl ShareNode {
    /// Binds with n0 discovery + relays: dialable by bare [`EndpointId`].
    pub async fn bind(identity: &DeviceIdentity) -> Result<Self> {
        Self::bind_with(identity, presets::N0).await
    }

    /// Binds without discovery or relays: peers must dial the full
    /// [`EndpointAddr`] carried in tickets. Offline/LAN use and tests.
    pub async fn bind_local(identity: &DeviceIdentity) -> Result<Self> {
        Self::bind_with(identity, presets::Minimal).await
    }

    async fn bind_with(identity: &DeviceIdentity, preset: impl presets::Preset) -> Result<Self> {
        let endpoint = Endpoint::builder(preset)
            .secret_key(identity.secret.clone())
            .bind()
            .await
            .context("binding iroh endpoint")?;
        let projects = Projects::default();
        let access_check = AccessCheck::default();
        let router = Router::builder(endpoint)
            .accept(
                ALPN,
                SyncProtocol {
                    projects: projects.clone(),
                    access_check: access_check.clone(),
                },
            )
            .spawn();
        Ok(Self {
            router,
            projects,
            access_check,
        })
    }

    /// This node's share identity.
    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }

    /// Installs (or replaces) the access check consulted before serving any
    /// project sync; a denied peer's stream is rejected. Belt-and-braces with
    /// app-level checks — build one from the capability layer with
    /// [`crate::auth::ProjectAuth::access_callback`].
    pub fn set_access_check(&self, check: AccessCheckFn) {
        *self.access_check.0.lock().unwrap() = Some(check);
    }

    /// Registers (or replaces) a shared project document.
    pub fn register(&self, project_id: impl Into<String>, doc: Automerge) {
        self.projects.lock().unwrap().insert(project_id.into(), doc);
    }

    /// Runs `f` on the registered document. `None` if the project is unknown.
    pub fn with_doc<R>(&self, project_id: &str, f: impl FnOnce(&mut Automerge) -> R) -> Option<R> {
        self.projects.lock().unwrap().get_mut(project_id).map(f)
    }

    /// A snapshot (fork) of the registered document. `None` if unknown.
    pub fn doc(&self, project_id: &str) -> Option<Automerge> {
        self.projects
            .lock()
            .unwrap()
            .get(project_id)
            .map(|d| d.fork())
    }

    /// A pasteable invite for a registered project, carrying this node's
    /// current address.
    pub fn ticket(&self, project_id: &str) -> Result<ShareTicket> {
        if !self.projects.lock().unwrap().contains_key(project_id) {
            return Err(anyerr!("project {project_id} is not registered"));
        }
        Ok(ShareTicket::new(self.router.endpoint().addr(), project_id))
    }

    /// Joins (or re-syncs) a shared project: dials the host in the ticket and
    /// runs one sync session. If this node doesn't have the project yet, it
    /// starts from an empty document.
    pub async fn join(&self, ticket: &ShareTicket) -> Result<(), JoinError> {
        self.projects
            .lock()
            .unwrap()
            .entry(ticket.project_id.clone())
            .or_default();
        let conn = self
            .router
            .endpoint()
            .connect(ticket.endpoint.endpoint_addr().clone(), ALPN)
            .await
            .context("dialing share host")?;
        let session = async {
            let (mut send, mut recv) = conn.open_bi().await.std_context("opening sync stream")?;
            send_frame(&mut send, ticket.project_id.as_bytes()).await?;
            run_sync(
                &self.projects,
                &ticket.project_id,
                &mut send,
                &mut recv,
                true,
            )
            .await
        };
        match session.await {
            Ok(()) => {
                // we received the last sync message, so we close; the responder awaits it.
                conn.close(0u32.into(), b"done");
                Ok(())
            }
            // vendor-edit: a REFUSED_CODE close from the host means the project is
            // unknown/denied there; surface it typed instead of as a stream error.
            Err(e) => match conn.close_reason() {
                Some(iroh::endpoint::ConnectionError::ApplicationClosed(c))
                    if c.error_code == REFUSED_CODE.into() =>
                {
                    Err(JoinError::Refused)
                }
                _ => Err(JoinError::Other(e)),
            },
        }
    }

    /// Graceful shutdown: stops accepting and closes the endpoint (flushing
    /// in-flight data).
    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await.std_context("router shutdown")
    }
}

// vendor-edit: typed join failure so callers can map a host refusal
// (unknown/denied project) to "not found" instead of a transport fault.
#[derive(Debug)]
pub enum JoinError {
    /// The host answered but refuses to serve this project.
    Refused,
    Other(AnyError),
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JoinError::Refused => write!(f, "host refused: share unknown or access denied"),
            JoinError::Other(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for JoinError {}

impl From<AnyError> for JoinError {
    fn from(e: AnyError) -> Self {
        JoinError::Other(e)
    }
}

// --- protocol handler ------------------------------------------------------

#[derive(Debug, Clone)]
struct SyncProtocol {
    projects: Projects,
    access_check: AccessCheck,
}

impl ProtocolHandler for SyncProtocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        // only resolves once the initiator has written the project-id frame.
        let (mut send, mut recv) = conn.accept_bi().await?;
        let project_id = recv_frame(&mut recv)
            .await?
            .ok_or_else(|| anyerr!("initiator sent empty project id"))?;
        let project_id = String::from_utf8(project_id).map_err(AcceptError::from_err)?;
        // vendor-edit: unknown/denied projects close with REFUSED_CODE so the
        // joining side sees a detectable refusal, not a generic handler error.
        if !self.access_check.allows(conn.remote_id(), &project_id)
            || !self.projects.lock().unwrap().contains_key(&project_id)
        {
            conn.close(REFUSED_CODE.into(), b"refused");
            return Ok(());
        }
        run_sync(&self.projects, &project_id, &mut send, &mut recv, false).await?;
        // we sent the last sync message: wait for the initiator to close so the
        // tail data isn't dropped.
        conn.closed().await;
        Ok(())
    }
}

// --- lockstep automerge sync -----------------------------------------------

/// One lockstep sync session over an open bidi stream: each round every side
/// sends one optional message and receives one; done when both send `None` in
/// the same round. `initiate` picks send-first vs receive-first.
async fn run_sync(
    projects: &Projects,
    project_id: &str,
    send: &mut SendStream,
    recv: &mut RecvStream,
    initiate: bool,
) -> Result<()> {
    // ponytail: sync a fork taken at session start so the registry lock is never
    // held across an await; local edits made mid-session ship on the next sync.
    let mut fork = projects
        .lock()
        .unwrap()
        .get(project_id)
        .ok_or_else(|| anyerr!("project {project_id} is not registered"))?
        .fork();
    let mut state = sync::State::new();
    let apply = |fork: &mut Automerge, state: &mut sync::State, msg: sync::Message| {
        fork.receive_sync_message(state, msg)
            .std_context("applying sync message")?;
        let mut projects = projects.lock().unwrap();
        let shared = projects
            .get_mut(project_id)
            .ok_or_else(|| anyerr!("project {project_id} vanished mid-sync"))?;
        shared.merge(fork).std_context("merging synced changes")?;
        Ok::<_, AnyError>(())
    };
    if initiate {
        for _ in 0..MAX_SYNC_ROUNDS {
            let our = fork.generate_sync_message(&mut state);
            let local_done = our.is_none();
            send_msg(our, send).await?;
            let their = recv_msg(recv).await?;
            let remote_done = their.is_none();
            if let Some(msg) = their {
                apply(&mut fork, &mut state, msg)?;
            }
            if local_done && remote_done {
                return Ok(());
            }
        }
    } else {
        for _ in 0..MAX_SYNC_ROUNDS {
            let their = recv_msg(recv).await?;
            let remote_done = their.is_none();
            if let Some(msg) = their {
                apply(&mut fork, &mut state, msg)?;
            }
            let our = fork.generate_sync_message(&mut state);
            let local_done = our.is_none();
            send_msg(our, send).await?;
            if local_done && remote_done {
                return Ok(());
            }
        }
    }
    // a peer can keep a session "alive" forever with non-converging traffic,
    // which no read timeout catches — cap the rounds instead.
    Err(anyerr!(
        "sync did not converge within {MAX_SYNC_ROUNDS} rounds"
    ))
}

// --- framing: 8-byte LE length prefix, 0 = "no message this round" ----------

async fn send_msg(msg: Option<sync::Message>, send: &mut SendStream) -> Result<()> {
    match msg {
        // an encoded sync message is never empty, so it can't alias the
        // zero-length "done" frame.
        Some(msg) => send_frame(send, &msg.encode()).await,
        None => send_frame(send, &[]).await,
    }
}

async fn recv_msg(recv: &mut RecvStream) -> Result<Option<sync::Message>> {
    match recv_frame(recv).await? {
        Some(buf) => Ok(Some(
            sync::Message::decode(&buf).std_context("decoding sync message")?,
        )),
        None => Ok(None),
    }
}

pub(crate) async fn send_frame(send: &mut SendStream, bytes: &[u8]) -> Result<()> {
    send.write_all(&(bytes.len() as u64).to_le_bytes())
        .await
        .std_context("writing frame")?;
    send.write_all(bytes).await.std_context("writing frame")?;
    Ok(())
}

/// `None` for a zero-length frame. The whole frame (length prefix + body)
/// must arrive within [`RECV_TIMEOUT`] — every protocol read in this crate
/// routes through here, so this is the single deadline for all of them.
pub(crate) async fn recv_frame(recv: &mut RecvStream) -> Result<Option<Vec<u8>>> {
    tokio::time::timeout(RECV_TIMEOUT, async {
        let mut len = [0u8; 8];
        recv.read_exact(&mut len)
            .await
            .std_context("reading frame length")?;
        let len = u64::from_le_bytes(len);
        if len == 0 {
            return Ok(None);
        }
        if len > MAX_FRAME {
            return Err(anyerr!("peer sent oversized frame ({len} bytes)"));
        }
        let mut buf = vec![0u8; len as usize];
        recv.read_exact(&mut buf)
            .await
            .std_context("reading frame body")?;
        Ok(Some(buf))
    })
    .await
    .map_err(|_| anyerr!("timed out waiting for peer frame"))?
}

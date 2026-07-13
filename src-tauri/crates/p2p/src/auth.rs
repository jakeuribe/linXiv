//! Capability layer (Phase 2): keyhive-backed per-project groups, delegation,
//! revocation, and content encryption, plus the cross-signed device binding
//! that ties an iroh endpoint to a keyhive individual.
//!
//! Future form: keyhive's `Local` futures are `!Send`, so this module uses
//! [`future_form::Sendable`] throughout — everything stays `Send` and plugs
//! into tokio and the [`crate::sync`] access-check hook without an actor.
//!
//! NB: keyhive uses ed25519-dalek 2.x while iroh uses 3.0-rc — different
//! crates. Key material only ever crosses that boundary as raw 32-byte arrays.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};

use beekem::{encrypted::EncryptedContent, error::CgkaError};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use future_form::Sendable;
use futures::lock::Mutex as AsyncMutex;
use iroh::EndpointId;
use keyhive_core::{
    access::Access,
    archive::Archive,
    contact_card::ContactCard,
    event::static_event::StaticEvent,
    keyhive::Keyhive,
    listener::no_listener::NoListener,
    principal::{
        document::{DecryptError as KhDecryptError, Document, id::DocumentId},
        group::{RevokeMemberError, id::GroupId},
        identifier::Identifier,
        membered::Membered,
        peer::Peer,
    },
    store::ciphertext::memory::MemoryCiphertextStore,
};
use keyhive_crypto::signer::memory::MemorySigner;
use n0_error::{AnyError, Result, anyerr};
use nonempty::nonempty;
use rand::{Rng as _, rngs::OsRng};
use serde::{Deserialize, Serialize};

use crate::sync::{AccessCheckFn, DeviceIdentity, KeyStoreError, seal, unseal, write_private};

// --- keyhive device identity -------------------------------------------------

/// Persistent keyhive signing identity: an Ed25519 seed stored on disk,
/// separate from (never shared with) the iroh transport key.
#[derive(Debug, Clone)]
pub struct AuthIdentity {
    signer: MemorySigner,
}

impl AuthIdentity {
    /// Loads the seed at `path`, generating and persisting a new one (0o600)
    /// if the file doesn't exist yet. The key IS the keyhive identity, so the
    /// same path always yields the same individual.
    pub fn load_or_generate(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let key = if path.exists() {
            let bytes = std::fs::read(path)?;
            let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("keyhive key file {} is not 32 bytes", path.display()),
                )
            })?;
            SigningKey::from_bytes(&seed)
        } else {
            let key = SigningKey::generate(&mut OsRng);
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            crate::sync::write_key(path, key.as_bytes())?;
            key
        };
        Ok(Self {
            signer: MemorySigner(key),
        })
    }

    /// The public half, which doubles as this device's keyhive member id.
    pub fn member_id(&self) -> MemberId {
        MemberId(self.signer.0.verifying_key().to_bytes())
    }
}

/// A keyhive individual, as raw Ed25519 verifying-key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemberId(pub [u8; 32]);

impl MemberId {
    fn identifier(&self) -> Result<Identifier> {
        let vk = VerifyingKey::from_bytes(&self.0)
            .map_err(|e| anyerr!("member id is not a valid Ed25519 key: {e}"))?;
        Ok(Identifier::from(vk))
    }
}

// --- cross-signed device binding ----------------------------------------------

/// Domain separator for binding statements; bump on layout changes.
const BINDING_CONTEXT: &[u8] = b"linxiv/device-binding/v0";

/// A cross-signed statement that one device controls both an iroh endpoint
/// and a keyhive individual: both public keys, each signing the pair.
///
/// A verified binding is how a peer maps `EndpointId -> keyhive member` for
/// access checks; neither key is ever reused across the two protocols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceBinding {
    iroh_id: [u8; 32],
    keyhive_vk: [u8; 32],
    // 64-byte Ed25519 signatures; Vec because serde arrays stop at 32.
    iroh_sig: Vec<u8>,
    keyhive_sig: Vec<u8>,
}

fn binding_statement(iroh_id: &[u8; 32], keyhive_vk: &[u8; 32]) -> Vec<u8> {
    let mut stmt = Vec::with_capacity(BINDING_CONTEXT.len() + 64);
    stmt.extend_from_slice(BINDING_CONTEXT);
    stmt.extend_from_slice(iroh_id);
    stmt.extend_from_slice(keyhive_vk);
    stmt
}

impl DeviceBinding {
    /// Signs the (iroh, keyhive) key pair with both secret keys.
    pub fn create(device: &DeviceIdentity, auth: &AuthIdentity) -> Self {
        let iroh_id = *device.endpoint_id().as_bytes();
        let keyhive_vk = auth.signer.0.verifying_key().to_bytes();
        let stmt = binding_statement(&iroh_id, &keyhive_vk);
        Self {
            iroh_sig: device.secret().sign(&stmt).to_bytes().to_vec(),
            keyhive_sig: auth.signer.0.sign(&stmt).to_bytes().to_vec(),
            iroh_id,
            keyhive_vk,
        }
    }

    /// Checks both signatures over the statement. `Ok(())` means whoever
    /// produced this binding controls both secret keys.
    pub fn verify(&self) -> Result<()> {
        let stmt = binding_statement(&self.iroh_id, &self.keyhive_vk);
        let iroh_sig: [u8; 64] = self
            .iroh_sig
            .as_slice()
            .try_into()
            .map_err(|_| anyerr!("iroh signature is not 64 bytes"))?;
        self.endpoint_id()?
            .verify(&stmt, &iroh::Signature::from_bytes(&iroh_sig))
            .map_err(|e| anyerr!("iroh signature check failed: {e}"))?;
        let keyhive_sig: [u8; 64] = self
            .keyhive_sig
            .as_slice()
            .try_into()
            .map_err(|_| anyerr!("keyhive signature is not 64 bytes"))?;
        let vk = VerifyingKey::from_bytes(&self.keyhive_vk)
            .map_err(|e| anyerr!("keyhive key is invalid: {e}"))?;
        vk.verify_strict(&stmt, &ed25519_dalek::Signature::from_bytes(&keyhive_sig))
            .map_err(|e| anyerr!("keyhive signature check failed: {e}"))?;
        Ok(())
    }

    /// The bound iroh endpoint.
    pub fn endpoint_id(&self) -> Result<EndpointId> {
        EndpointId::from_bytes(&self.iroh_id).map_err(|e| anyerr!("iroh key is invalid: {e}"))
    }

    /// The bound keyhive individual.
    pub fn member_id(&self) -> MemberId {
        MemberId(self.keyhive_vk)
    }

    /// Serializes for transport/storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("DeviceBinding serialization cannot fail")
    }

    /// Inverse of [`Self::to_bytes`]. Does NOT verify; call [`Self::verify`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        postcard::from_bytes(bytes).map_err(|e| anyerr!("malformed device binding: {e}"))
    }
}

// --- roles ---------------------------------------------------------------------

/// Access level for a project member; maps 1:1 onto keyhive's `Access`.
/// `Edit` implies `Read`; `Relay` may forward ciphertexts but not read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Relay,
    Read,
    Edit,
    Admin,
}

impl From<Role> for Access {
    fn from(role: Role) -> Access {
        match role {
            Role::Relay => Access::Relay,
            Role::Read => Access::Read,
            Role::Edit => Access::Edit,
            Role::Admin => Access::Admin,
        }
    }
}

impl From<Access> for Role {
    fn from(access: Access) -> Role {
        match access {
            Access::Relay => Role::Relay,
            Access::Read => Role::Read,
            Access::Edit => Role::Edit,
            Access::Admin => Role::Admin,
        }
    }
}

// --- project capability state ----------------------------------------------------

/// Decrypt failure; `KeyNotFound` is the cryptographic "you are not (or no
/// longer) in this epoch" signal.
#[derive(Debug, PartialEq, Eq)]
pub enum DecryptError {
    /// The project id has no local doc (create, or ingest + adopt, first).
    UnknownProject,
    /// No key material for the content's epoch: revoked, or pre-join content.
    KeyNotFound,
    /// Anything else (malformed sealed bytes, cipher failure, ...).
    Other(String),
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecryptError::UnknownProject => write!(f, "unknown project"),
            DecryptError::KeyNotFound => write!(f, "no key for this content's epoch"),
            DecryptError::Other(e) => write!(f, "decrypt failed: {e}"),
        }
    }
}

impl std::error::Error for DecryptError {}

// vendor-edit: typed [`ProjectAuth::set_role`] failure, wrapped as the
// direct [`AnyError`] payload so callers can `downcast_ref::<SetRoleError>()`
// it (same pattern as beelay's `BlobError`).
#[derive(Debug, PartialEq, Eq)]
pub enum SetRoleError {
    /// The revoke leg would remove the doc's last CGKA reader
    /// (`CgkaError::RemoveLastMember`); a doc must keep at least one.
    LastReader,
}

impl std::fmt::Display for SetRoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetRoleError::LastReader => {
                write!(f, "role change would remove the project's last reader")
            }
        }
    }
}

impl std::error::Error for SetRoleError {}

type Kh = Keyhive<
    Sendable,
    MemorySigner,
    [u8; 32],
    Vec<u8>,
    MemoryCiphertextStore<[u8; 32], Vec<u8>>,
    NoListener,
    OsRng,
>;

type DocHandle = Arc<AsyncMutex<Document<Sendable, MemorySigner, [u8; 32], NoListener>>>;

// write_private (tmp writer for secret-carrying files) moved to crate::sync
// so the encrypted device-key migration shares it.

#[cfg(unix)]
fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn fsync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Blocking tmp+rename+fsync of `bytes` to `dir/state.bin`; run off the async executor.
fn write_state_file(dir: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join("state.bin.tmp");
    write_private(&tmp, bytes).and_then(|f| f.sync_all())?;
    std::fs::rename(&tmp, dir.join("state.bin"))?;
    fsync_dir(dir)
}

/// First byte of state.bin; the postcard payload follows.
const STATE_FORMAT_VERSION: u8 = 1;

// vendor-edit: encrypted state at rest (write-enforcement spec §8).
/// state.bin v2 = DEK-encrypted: version byte, then [`seal`] output
/// (24-byte nonce + XChaCha20-Poly1305 ciphertext of the postcard payload).
const STATE_FORMAT_VERSION_ENCRYPTED: u8 = 2;

/// Combined archive + project registry, persisted as one file.
#[derive(Serialize, Deserialize)]
struct PersistedState {
    archive: Archive<[u8; 32]>,
    projects: Vec<(String, Option<[u8; 32]>, [u8; 32])>,
}

#[derive(Clone, Copy)]
struct ProjectIds {
    /// Membership group; `None` when the project was adopted without one.
    group: Option<GroupId>,
    doc: DocumentId,
}

/// One device's capability state: a keyhive instance plus the
/// `project id -> (group, doc)` registry.
///
/// Content keys live on the doc; membership is managed on the group (which is
/// an admin coparent of the doc, so grants propagate transitively).
pub struct ProjectAuth {
    keyhive: Kh,
    projects: StdMutex<HashMap<String, ProjectIds>>,
    /// Set by [`Self::load_or_new`]; mutating ops write state back here.
    persist_dir: Option<PathBuf>,
    /// Set by [`Self::load_or_new_with_dek`]; [`Self::persist`] seals
    /// state.bin with it (`None` = plaintext v1).
    dek: Option<[u8; 32]>,
    /// Serializes [`Self::persist`]'s snapshot+write.
    persist_lock: AsyncMutex<()>,
    /// Serializes membership transitions ([`Self::set_role`],
    /// [`Self::revoke_member`]) against [`Self::encrypt`], so a
    /// flush-triggered encrypt can never land between a transition's legs
    /// and seal content in an epoch the member is keyed out of.
    transition_lock: AsyncMutex<()>,
}

impl std::fmt::Debug for ProjectAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProjectAuth({:?})", self.member_id())
    }
}

impl ProjectAuth {
    /// A fresh in-memory keyhive instance over the device's persistent auth
    /// identity; nothing survives drop. Use [`Self::load_or_new`] on devices.
    pub async fn new(identity: &AuthIdentity) -> Result<Self> {
        let keyhive = Kh::generate(
            identity.signer.clone(),
            MemoryCiphertextStore::new(),
            NoListener,
            OsRng,
        )
        .await
        .map_err(|e| anyerr!("generating keyhive instance: {e}"))?;
        Ok(Self {
            keyhive,
            projects: StdMutex::new(HashMap::new()),
            persist_dir: None,
            dek: None,
            persist_lock: AsyncMutex::new(()),
            transition_lock: AsyncMutex::new(()),
        })
    }

    // vendor-edit: keyhive state persistence (archive + project registry).

    /// Restores state persisted under `dir` (starting fresh if the state
    /// file is absent) and auto-persists there after every mutating op.
    /// An undecodable state file is an error.
    pub async fn load_or_new(identity: &AuthIdentity, dir: &Path) -> Result<Self> {
        Self::load_or_new_with_dek(identity, dir, None).await
    }

    // vendor-edit: encrypted state at rest (write-enforcement spec §8).
    /// Like [`Self::load_or_new`], but with `Some(dek)` state.bin is sealed
    /// (XChaCha20-Poly1305) under the DEK; v1 plaintext state is re-persisted
    /// encrypted on load. Encrypted state loaded without the right DEK fails
    /// with a downcastable [`KeyStoreError`].
    pub async fn load_or_new_with_dek(
        identity: &AuthIdentity,
        dir: &Path,
        dek: Option<&[u8; 32]>,
    ) -> Result<Self> {
        let state_path = dir.join("state.bin");
        if !state_path.exists() {
            std::fs::create_dir_all(dir).map_err(|e| anyerr!("creating {dir:?}: {e}"))?;
            let mut auth = Self::new(identity).await?;
            auth.persist_dir = Some(dir.to_owned());
            auth.dek = dek.copied();
            return Ok(auth);
        }
        let bytes =
            std::fs::read(&state_path).map_err(|e| anyerr!("reading keyhive state: {e}"))?;
        let (version, rest) = bytes
            .split_first()
            .ok_or_else(|| anyerr!("unknown state.bin format version"))?;
        let payload = match (*version, dek) {
            (STATE_FORMAT_VERSION, _) => rest.to_vec(),
            (STATE_FORMAT_VERSION_ENCRYPTED, Some(dek)) => {
                unseal(dek, rest).map_err(AnyError::from_std)?
            }
            (STATE_FORMAT_VERSION_ENCRYPTED, None) => {
                return Err(AnyError::from_std(KeyStoreError::Locked));
            }
            _ => return Err(anyerr!("unknown state.bin format version")),
        };
        let state: PersistedState =
            postcard::from_bytes(&payload).map_err(|e| anyerr!("malformed keyhive state: {e}"))?;
        // The ciphertext store is not archived; a fresh one is passed here.
        let keyhive = Kh::try_from_archive(
            &state.archive,
            identity.signer.clone(),
            MemoryCiphertextStore::new(),
            NoListener,
            Arc::new(AsyncMutex::new(OsRng)),
        )
        .await
        .map_err(|e| anyerr!("restoring keyhive archive: {e}"))?;
        let mut projects = HashMap::new();
        for (id, group, doc) in state.projects {
            projects.insert(
                id,
                ProjectIds {
                    group: match group {
                        Some(g) => Some(GroupId::new(MemberId(g).identifier()?)),
                        None => None,
                    },
                    doc: DocumentId::from(MemberId(doc).identifier()?),
                },
            );
        }
        let auth = Self {
            keyhive,
            projects: StdMutex::new(projects),
            persist_dir: Some(dir.to_owned()),
            dek: dek.copied(),
            persist_lock: AsyncMutex::new(()),
            transition_lock: AsyncMutex::new(()),
        };
        // v1 plaintext loaded with a DEK: re-persist encrypted (one-time migration).
        if *version == STATE_FORMAT_VERSION && auth.dek.is_some() {
            auth.persist().await?;
        }
        Ok(auth)
    }

    /// Writes state.bin under the [`Self::load_or_new`] dir via tmp+rename,
    /// fsyncing the tmp file before rename and the parent dir after;
    /// no-op for in-memory instances.
    async fn persist(&self) -> Result<()> {
        let Some(dir) = &self.persist_dir else {
            return Ok(());
        };
        let _guard = self.persist_lock.lock().await;
        // ponytail: whole-state rewrite per mutation; incremental event log
        // if archive size ever matters.
        let state = PersistedState {
            archive: self.keyhive.into_archive().await,
            projects: self
                .projects
                .lock()
                .unwrap()
                .iter()
                .map(|(id, p)| (id.clone(), p.group.map(|g| g.to_bytes()), p.doc.to_bytes()))
                .collect(),
        };
        let plain =
            postcard::to_stdvec(&state).map_err(|e| anyerr!("encoding keyhive state: {e}"))?;
        let bytes = match &self.dek {
            Some(dek) => {
                let mut bytes = vec![STATE_FORMAT_VERSION_ENCRYPTED];
                bytes.extend(seal(dek, &plain));
                bytes
            }
            None => {
                let mut bytes = vec![STATE_FORMAT_VERSION];
                bytes.extend(plain);
                bytes
            }
        };
        // Bounded retries: a transient write failure here would otherwise leave
        // in-memory state (e.g. post-revoke key rotation) mutated but unpersisted.
        let mut last_err = None;
        for attempt in 1..=3u32 {
            let dir = dir.clone();
            let bytes = bytes.clone();
            match tokio::task::spawn_blocking(move || write_state_file(&dir, &bytes)).await {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(e)) => last_err = Some(anyerr!("writing state.bin: {e}")),
                Err(e) => last_err = Some(anyerr!("persist task panicked: {e}")),
            }
            if attempt < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(50 * attempt as u64)).await;
            }
        }
        Err(last_err.expect("loop always sets last_err before exiting"))
    }

    /// This device's keyhive member id.
    pub fn member_id(&self) -> MemberId {
        MemberId(self.keyhive.id().to_bytes())
    }

    /// Serializable card other devices ingest to learn this identity.
    pub async fn contact_card(&self) -> Result<Vec<u8>> {
        let card = self.keyhive.get_existing_contact_card().await;
        postcard::to_stdvec(&card).map_err(|e| anyerr!("encoding contact card: {e}"))
    }

    /// Registers a peer from its [`Self::contact_card`] bytes, returning its
    /// member id (usable in [`Self::add_member`]).
    pub async fn receive_contact_card(&self, bytes: &[u8]) -> Result<MemberId> {
        let card: ContactCard =
            postcard::from_bytes(bytes).map_err(|e| anyerr!("malformed contact card: {e}"))?;
        self.keyhive
            .receive_contact_card(&card)
            .await
            .map_err(|e| anyerr!("ingesting contact card: {e}"))?;
        self.persist().await?;
        Ok(MemberId(card.id().to_bytes()))
    }

    /// Creates the capability group + encrypted doc for a new project this
    /// device hosts. This device becomes the group's admin.
    /// Requires a multi-thread tokio runtime (uses `block_in_place`).
    pub async fn create_project(&self, project_id: &str) -> Result<()> {
        if self.projects.lock().unwrap().contains_key(project_id) {
            return Err(anyerr!("project {project_id} already exists"));
        }
        // Keyhive's group/doc generation signs with an ephemeral
        // `Box<dyn SyncSignerBasic>` (no `Send` bound upstream), so its future
        // is `!Send` despite the `Sendable` form. Run it to completion on a
        // scoped thread so this method's future stays `Send`.
        let (group_id, doc_id) = tokio::task::block_in_place(|| {
            std::thread::scope(|s| {
                s.spawn(|| {
                    futures::executor::block_on(async {
                        let group = self
                            .keyhive
                            .generate_group(vec![])
                            .await
                            .map_err(|e| anyerr!("creating project group: {e}"))?;
                        let group_id = { group.lock().await.group_id() };
                        let doc = self
                            .keyhive
                            .generate_doc(vec![Peer::Group(group_id, group)], nonempty![[0u8; 32]])
                            .await
                            .map_err(|e| anyerr!("creating project doc: {e}"))?;
                        let doc_id = { doc.lock().await.doc_id() };
                        Ok::<_, n0_error::AnyError>((group_id, doc_id))
                    })
                })
                .join()
                .expect("keyhive generate thread panicked")
            })
        })?;
        {
            // Lock was dropped during keyhive generation; re-check before insert.
            let mut projects = self.projects.lock().unwrap();
            if projects.contains_key(project_id) {
                return Err(anyerr!("project {project_id} already exists"));
            }
            projects.insert(
                project_id.to_owned(),
                ProjectIds {
                    group: Some(group_id),
                    doc: doc_id,
                },
            );
        }
        if let Err(e) = self.persist().await {
            self.projects.lock().unwrap().remove(project_id);
            eprintln!(
                "create_project: {project_id} group/doc now orphaned in the keyhive archive (no delete API): {e}"
            );
            return Err(e);
        }
        Ok(())
    }

    /// The project doc's id — ship it in invites so joiners can
    /// [`Self::adopt_project`].
    pub fn doc_id(&self, project_id: &str) -> Option<[u8; 32]> {
        self.projects
            .lock()
            .unwrap()
            .get(project_id)
            .map(|p| p.doc.to_bytes())
    }

    /// The project's membership group id, for invites — `None` for unknown
    /// projects or ones adopted without a group.
    pub fn group_id(&self, project_id: &str) -> Option<[u8; 32]> {
        self.projects
            .lock()
            .unwrap()
            .get(project_id)
            .and_then(|p| p.group.map(|g| g.to_bytes()))
    }

    /// All project ids this device knows (has created or adopted).
    pub fn known_project_ids(&self) -> Vec<String> {
        self.projects.lock().unwrap().keys().cloned().collect()
    }

    /// Maps `project_id` onto a doc learned via [`Self::ingest_events`]
    /// (from an invite). Fails if the doc's events haven't been ingested yet.
    /// With the host's [`Self::group_id`], membership management works here
    /// too (subject to keyhive's own delegation rules).
    pub async fn adopt_project(
        &self,
        project_id: &str,
        doc_id: [u8; 32],
        group_id: Option<[u8; 32]>,
    ) -> Result<()> {
        let doc_id = DocumentId::from(MemberId(doc_id).identifier()?);
        if self.keyhive.get_document(doc_id).await.is_none() {
            return Err(anyerr!(
                "unknown doc for project {project_id}; ingest the invite events first"
            ));
        }
        let group = match group_id {
            Some(g) => {
                let group_id = GroupId::new(MemberId(g).identifier()?);
                if self.keyhive.get_group(group_id).await.is_none() {
                    return Err(anyerr!(
                        "unknown group for project {project_id}; ingest the invite events first"
                    ));
                }
                Some(group_id)
            }
            None => None,
        };
        {
            // atomic check-and-insert.
            let mut projects = self.projects.lock().unwrap();
            if projects.contains_key(project_id) {
                return Err(anyerr!("project {project_id} already exists"));
            }
            projects.insert(project_id.to_owned(), ProjectIds { group, doc: doc_id });
        }
        if let Err(e) = self.persist().await {
            self.projects.lock().unwrap().remove(project_id);
            return Err(e);
        }
        Ok(())
    }

    fn project_ids(&self, project_id: &str) -> Result<ProjectIds> {
        self.projects
            .lock()
            .unwrap()
            .get(project_id)
            .copied()
            .ok_or_else(|| anyerr!("unknown project {project_id}"))
    }

    async fn doc_handle(&self, project_id: &str) -> Result<DocHandle> {
        let ids = self.project_ids(project_id)?;
        self.keyhive
            .get_document(ids.doc)
            .await
            .ok_or_else(|| anyerr!("doc for project {project_id} vanished"))
    }

    async fn group_membered(
        &self,
        project_id: &str,
    ) -> Result<Membered<Sendable, MemorySigner, [u8; 32], NoListener>> {
        let group_id = self
            .project_ids(project_id)?
            .group
            .ok_or_else(|| anyerr!("this device does not manage membership of {project_id}"))?;
        let group = self
            .keyhive
            .get_group(group_id)
            .await
            .ok_or_else(|| anyerr!("group for project {project_id} vanished"))?;
        Ok(Membered::Group(group_id, group))
    }

    /// Grants `member` (known via contact card) `role` on the project: signs
    /// a delegation into the group and extends the doc's key tree to them.
    pub async fn add_member(&self, project_id: &str, member: MemberId, role: Role) -> Result<()> {
        let agent = self
            .keyhive
            .get_agent(member.identifier()?)
            .await
            .ok_or_else(|| anyerr!("unknown member; exchange contact cards first"))?;
        let group = self.group_membered(project_id).await?;
        self.keyhive
            .add_member(agent, &group, role.into(), &[])
            .await
            .map_err(|e| anyerr!("adding member: {e}"))?;
        self.persist().await?;
        Ok(())
    }

    /// Revokes `member` and rotates the doc key (PCS update), so content
    /// encrypted from now on is undecryptable to them. Content from epochs
    /// they belonged to stays readable to them forever — by design.
    pub async fn revoke_member(&self, project_id: &str, member: MemberId) -> Result<()> {
        // No encrypt between revoke and rotation: content sealed in that
        // window would land in the pre-rotation epoch the revokee still
        // holds, weakening the "undecryptable from now on" promise.
        let _transition = self.transition_lock.lock().await;
        let group = self.group_membered(project_id).await?;
        self.keyhive
            .revoke_member(member.identifier()?, true, &group)
            .await
            .map_err(|e| anyerr!("revoking member: {e}"))?;
        let doc = self.doc_handle(project_id).await?;
        self.keyhive
            .force_pcs_update(doc)
            .await
            .map_err(|e| anyerr!("rotating project key: {e}"))?;
        if let Err(e) = self.persist().await {
            eprintln!(
                "revoke_member: key rotation for {project_id} is live in-memory but not yet durable: {e}"
            );
            return Err(e);
        }
        Ok(())
    }

    // vendor-edit: role transitions (write-enforcement spec §3). Co-admin
    // note: granting Admin is keyhive-supported and a valid target here, but
    // app-deferred — management ops that need PCS rotation (revoke,
    // downgrade) must run where the doc is hosted (spec §1.1), so the app
    // keeps membership management Hoster-only for now.

    /// Moves `member` to `role`: a no-op when already at `role`, a plain
    /// grant ([`Self::add_member`]) from no role, and otherwise revoke +
    /// re-grant, keeping a SINGLE live delegation per member — keyhive's
    /// doc-level access resolution picks between stacked delegations by
    /// digest order, so layering a second grant would turn
    /// [`Self::query_access`] into a coin flip.
    ///
    /// A downgrade eagerly rotates the doc key (PCS update), an upgrade
    /// skips it and the member keeps reading everything they could before.
    /// Ordering is load-bearing twice over: the re-grant runs right AFTER
    /// the revoke (revocation kills all of the member's delegations, a
    /// fresh grant included) and BEFORE any rotation — content encrypted
    /// while the member is keyed out of the tree would be permanently
    /// unreadable to them, so no such window may exist. Belt and braces,
    /// the whole transition also holds `transition_lock`, which
    /// [`Self::encrypt`] takes too, so a concurrent flush-triggered encrypt
    /// (e.g. [`crate::beelay`]'s inbound-dial flush) cannot interleave with
    /// the legs at all.
    pub async fn set_role(&self, project_id: &str, member: MemberId, role: Role) -> Result<()> {
        let current = match self.query_access(project_id, member).await? {
            Some(current) if current == role => return Ok(()),
            // no delegation yet: a plain grant, not a transition.
            None => return self.add_member(project_id, member, role).await,
            Some(current) => current,
        };
        let _transition = self.transition_lock.lock().await;
        let group = self.group_membered(project_id).await?;
        self.keyhive
            .revoke_member(member.identifier()?, true, &group)
            .await
            .map_err(|e| match e {
                RevokeMemberError::CgkaError(CgkaError::RemoveLastMember) => {
                    AnyError::from_std(SetRoleError::LastReader)
                }
                e => anyerr!("revoking member for role change: {e}"),
            })?;
        // add_member persists, making revoke + re-grant durable in one
        // write. A failure (or cancellation) between the legs leaves the
        // member revoked but the epoch UNrotated — nothing they need is
        // sealed away, and re-running set_role recovers them in full.
        self.add_member(project_id, member, role).await?;
        if role < current {
            let doc = self.doc_handle(project_id).await?;
            self.keyhive
                .force_pcs_update(doc)
                .await
                .map_err(|e| anyerr!("rotating project key: {e}"))?;
            // rotation mints CGKA ops; persist like revoke_member does. A
            // failure here leaves the role change durable and only the
            // best-effort eager rotation unpersisted.
            self.persist().await?;
        }
        Ok(())
    }

    /// The member's effective access on the project (via group or direct),
    /// `None` if they have no delegation at all.
    pub async fn query_access(&self, project_id: &str, member: MemberId) -> Result<Option<Role>> {
        let doc = self.doc_handle(project_id).await?;
        let id = member.identifier()?;
        let members = {
            let locked = doc.lock().await;
            locked.transitive_members().await
        };
        Ok(members.get(&id).map(|(_, access)| Role::from(*access)))
    }

    /// Encrypts `content` under the project doc's current epoch key.
    pub async fn encrypt(&self, project_id: &str, content: &[u8]) -> Result<Vec<u8>> {
        // serialized against set_role/revoke_member (see set_role's docs).
        let _transition = self.transition_lock.lock().await;
        let doc = self.doc_handle(project_id).await?;
        // ponytail: random content ref, no causal predecessors — real automerge
        // change hashes take over as refs in Phase 3.
        let content_ref: [u8; 32] = rand::thread_rng().r#gen();
        let sealed = self
            .keyhive
            .try_encrypt_content(doc, &content_ref, &vec![], content)
            .await
            .map_err(|e| anyerr!("encrypting content: {e}"))?;
        // encryption can mint CGKA ops; without them archived, this ciphertext
        // is undecryptable after a restart.
        self.persist().await?;
        postcard::to_stdvec(sealed.encrypted_content())
            .map_err(|e| anyerr!("encoding sealed content: {e}"))
    }

    /// Decrypts [`Self::encrypt`] output, provided this device holds key
    /// material for the content's epoch.
    pub async fn decrypt(
        &self,
        project_id: &str,
        sealed: &[u8],
    ) -> std::result::Result<Vec<u8>, DecryptError> {
        let ids = self
            .projects
            .lock()
            .unwrap()
            .get(project_id)
            .copied()
            .ok_or(DecryptError::UnknownProject)?;
        let doc = self
            .keyhive
            .get_document(ids.doc)
            .await
            .ok_or(DecryptError::UnknownProject)?;
        let sealed: EncryptedContent<Vec<u8>, [u8; 32]> =
            postcard::from_bytes(sealed).map_err(|e| DecryptError::Other(e.to_string()))?;
        self.keyhive
            .try_decrypt_content(doc, &sealed)
            .await
            .map_err(|e| match e {
                KhDecryptError::KeyNotFound => DecryptError::KeyNotFound,
                other => DecryptError::Other(other.to_string()),
            })
    }

    /// Everything `member` is authorized to see (visibility-filtered static
    /// events: delegations, revocations, key ops), as bytes to ship to them.
    /// These bytes — plus the host's `EndpointId` and the project's
    /// [`Self::doc_id`] — are what an invite carries.
    pub async fn export_events_for(&self, member: MemberId) -> Result<Vec<u8>> {
        let agent = self
            .keyhive
            .get_agent(member.identifier()?)
            .await
            .ok_or_else(|| anyerr!("unknown member; exchange contact cards first"))?;
        let events = self.keyhive.static_events_for_agent(&agent).await;
        let events: Vec<StaticEvent<[u8; 32]>> = events.into_values().collect();
        postcard::to_stdvec(&events).map_err(|e| anyerr!("encoding events: {e}"))
    }

    /// Ingests a peer's [`Self::export_events_for`] bytes. Errs if any events
    /// remain stuck on missing dependencies (i.e. the export was incomplete).
    pub async fn ingest_events(&self, bytes: &[u8]) -> Result<()> {
        let events: Vec<StaticEvent<[u8; 32]>> =
            postcard::from_bytes(bytes).map_err(|e| anyerr!("malformed event bytes: {e}"))?;
        let stuck = self.keyhive.ingest_unsorted_static_events(events).await;
        if !stuck.is_empty() {
            return Err(anyerr!(
                "{} events still stuck on missing dependencies",
                stuck.len()
            ));
        }
        self.persist().await?;
        Ok(())
    }

    /// Builds a [`crate::ShareNode::set_access_check`] callback from current
    /// keyhive membership plus verified device bindings: a peer endpoint may
    /// sync a project iff its bound keyhive member can read that project.
    /// Unverifiable bindings and unbound endpoints are denied.
    pub async fn access_callback(&self, bindings: &[DeviceBinding]) -> AccessCheckFn {
        // ponytail: snapshot allowlist — rebuild and re-set after membership or
        // binding changes. Live keyhive queries need an async bridge; add one
        // when grants must take effect without re-installing the callback.
        let snapshot: Vec<(String, DocumentId)> = self
            .projects
            .lock()
            .unwrap()
            .iter()
            .map(|(id, p)| (id.clone(), p.doc))
            .collect();
        let mut allowed: HashSet<([u8; 32], String)> = HashSet::new();
        for binding in bindings {
            if binding.verify().is_err() {
                continue;
            }
            let Ok(member) = binding.member_id().identifier() else {
                continue;
            };
            for (project_id, doc_id) in &snapshot {
                let Some(doc) = self.keyhive.get_document(*doc_id).await else {
                    continue;
                };
                let members = {
                    let locked = doc.lock().await;
                    locked.transitive_members().await
                };
                if members
                    .get(&member)
                    .is_some_and(|(_, access)| access.is_reader())
                {
                    allowed.insert((binding.iroh_id, project_id.clone()));
                }
            }
        }
        Arc::new(move |peer: EndpointId, project_id: &str| {
            allowed.contains(&(*peer.as_bytes(), project_id.to_owned()))
        })
    }
}

// The `Sendable` future-form choice must keep ProjectAuth usable from tokio
// tasks; this fails to compile if a keyhive type change breaks that.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ProjectAuth>();
};

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
    path::Path,
    sync::{Arc, Mutex as StdMutex},
};

use beekem::encrypted::EncryptedContent;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use future_form::Sendable;
use futures::lock::Mutex as AsyncMutex;
use iroh::EndpointId;
use keyhive_core::{
    access::Access,
    contact_card::ContactCard,
    event::static_event::StaticEvent,
    keyhive::Keyhive,
    listener::no_listener::NoListener,
    principal::{
        document::{DecryptError as KhDecryptError, Document, id::DocumentId},
        group::id::GroupId,
        identifier::Identifier,
        membered::Membered,
        peer::Peer,
    },
    store::ciphertext::memory::MemoryCiphertextStore,
};
use keyhive_crypto::signer::memory::MemorySigner;
use n0_error::{Result, anyerr};
use nonempty::nonempty;
use rand::{Rng as _, rngs::OsRng};
use serde::{Deserialize, Serialize};

use crate::sync::{AccessCheckFn, DeviceIdentity};

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

#[derive(Clone, Copy)]
struct ProjectIds {
    /// Membership group; `None` on adopters, who can't manage membership.
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
}

impl std::fmt::Debug for ProjectAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProjectAuth({:?})", self.member_id())
    }
}

impl ProjectAuth {
    /// A fresh keyhive instance over the device's persistent auth identity.
    pub async fn new(identity: &AuthIdentity) -> Result<Self> {
        // ponytail: in-memory state only — every restart replays membership via
        // event exchange. Persist with into_archive/try_from_archive when
        // offline-first startup matters.
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
        })
    }

    /// This device's keyhive member id.
    pub fn member_id(&self) -> MemberId {
        MemberId(self.keyhive.id().to_bytes())
    }

    /// Serializable card other devices ingest to learn this identity.
    pub async fn contact_card(&self) -> Result<Vec<u8>> {
        let card = self
            .keyhive
            .contact_card()
            .await
            .map_err(|e| anyerr!("creating contact card: {e}"))?;
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
        Ok(MemberId(card.id().to_bytes()))
    }

    /// Creates the capability group + encrypted doc for a new project this
    /// device hosts. This device becomes the group's admin.
    pub async fn create_project(&self, project_id: &str) -> Result<()> {
        if self.projects.lock().unwrap().contains_key(project_id) {
            return Err(anyerr!("project {project_id} already exists"));
        }
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
        self.projects.lock().unwrap().insert(
            project_id.to_owned(),
            ProjectIds {
                group: Some(group_id),
                doc: doc_id,
            },
        );
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

    /// Maps `project_id` onto a doc learned via [`Self::ingest_events`]
    /// (from an invite). Fails if the doc's events haven't been ingested yet.
    pub async fn adopt_project(&self, project_id: &str, doc_id: [u8; 32]) -> Result<()> {
        let vk = VerifyingKey::from_bytes(&doc_id)
            .map_err(|e| anyerr!("doc id is not a valid identifier: {e}"))?;
        let doc_id = DocumentId::from(Identifier::from(vk));
        if self.keyhive.get_document(doc_id).await.is_none() {
            return Err(anyerr!(
                "unknown doc for project {project_id}; ingest the invite events first"
            ));
        }
        // atomic check-and-insert: never silently remap an existing project.
        let mut projects = self.projects.lock().unwrap();
        if projects.contains_key(project_id) {
            return Err(anyerr!("project {project_id} already exists"));
        }
        projects.insert(
            project_id.to_owned(),
            ProjectIds {
                group: None,
                doc: doc_id,
            },
        );
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
        Ok(())
    }

    /// Revokes `member` and rotates the doc key (PCS update), so content
    /// encrypted from now on is undecryptable to them. Content from epochs
    /// they belonged to stays readable to them forever — by design.
    pub async fn revoke_member(&self, project_id: &str, member: MemberId) -> Result<()> {
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
        let doc = self.doc_handle(project_id).await?;
        // ponytail: random content ref, no causal predecessors — real automerge
        // change hashes take over as refs in Phase 3.
        let content_ref: [u8; 32] = rand::thread_rng().r#gen();
        let sealed = self
            .keyhive
            .try_encrypt_content(doc, &content_ref, &vec![], content)
            .await
            .map_err(|e| anyerr!("encrypting content: {e}"))?;
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

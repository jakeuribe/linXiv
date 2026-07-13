#![cfg(feature = "auth-keyhive")]

use anyhow::Result;
use automerge::{Automerge, ROOT, transaction::Transactable};
use linxiv_p2p::{
    DeviceIdentity, KeyStoreError, ShareNode,
    auth::{AuthIdentity, DecryptError, DeviceBinding, ProjectAuth, Role},
};

fn identities(dir: &std::path::Path, name: &str) -> (DeviceIdentity, AuthIdentity) {
    let device = DeviceIdentity::load_or_generate(dir.join(format!("{name}.iroh.key"))).unwrap();
    let auth = AuthIdentity::load_or_generate(dir.join(format!("{name}.keyhive.key"))).unwrap();
    (device, auth)
}

#[test]
fn auth_identity_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kh.key");
    let first = AuthIdentity::load_or_generate(&path).unwrap();
    let second = AuthIdentity::load_or_generate(&path).unwrap();
    assert_eq!(first.member_id(), second.member_id());
}

#[test]
fn dual_key_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (device, auth) = identities(dir.path(), "dev");

    let binding = DeviceBinding::create(&device, &auth);
    binding.verify().expect("fresh binding verifies");
    assert_eq!(binding.endpoint_id().unwrap(), device.endpoint_id());
    assert_eq!(binding.member_id(), auth.member_id());

    // round-trips through bytes
    let bytes = binding.to_bytes();
    let parsed = DeviceBinding::from_bytes(&bytes).unwrap();
    assert_eq!(parsed, binding);
    parsed.verify().expect("parsed binding verifies");

    // tampering with any byte breaks decode or verification
    for i in 0..bytes.len() {
        let mut tampered = bytes.clone();
        tampered[i] ^= 0x01;
        let still_valid = DeviceBinding::from_bytes(&tampered)
            .and_then(|b| b.verify())
            .is_ok();
        assert!(!still_valid, "tampered byte {i} still verified");
    }
}

/// Alice creates a project, grants bob Edit via contact card, encrypts, and
/// ships an "invite" (endpoint id + project/doc/group ids + delegation
/// events); bob ingests,
/// adopts, and decrypts. Returns the live state for the revocation test.
async fn grant_flow() -> Result<(ProjectAuth, ProjectAuth, Vec<u8>)> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth) = identities(dir.path(), "alice");
    let (_, bob_auth) = identities(dir.path(), "bob");
    let alice = ProjectAuth::new(&alice_auth).await?;
    let bob = ProjectAuth::new(&bob_auth).await?;

    alice.create_project("proj").await?;

    let bob_id = alice
        .receive_contact_card(&bob.contact_card().await?)
        .await?;
    assert_eq!(bob_id, bob_auth.member_id());
    alice.add_member("proj", bob_id, Role::Edit).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, Some(Role::Edit));

    let sealed = alice.encrypt("proj", b"secret plan").await?;

    // an invite = hoster endpoint + project/doc/group ids + delegation events.
    let invite = postcard::to_stdvec(&(
        *alice_device.endpoint_id().as_bytes(),
        "proj",
        alice.doc_id("proj").unwrap(),
        alice.group_id("proj").unwrap(),
        alice.export_events_for(bob_id).await?,
    ))?;

    let (_host, project_id, doc_id, group_id, events): (
        [u8; 32],
        String,
        [u8; 32],
        [u8; 32],
        Vec<u8>,
    ) = postcard::from_bytes(&invite)?;
    bob.ingest_events(&events).await?; // Err if any events are stuck
    bob.adopt_project(&project_id, doc_id, Some(group_id))
        .await?;

    let plain = bob.decrypt(&project_id, &sealed).await?;
    assert_eq!(plain, b"secret plan");

    Ok((alice, bob, sealed))
}

#[tokio::test(flavor = "multi_thread")]
async fn grant_and_decrypt() -> Result<()> {
    grant_flow().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_blocks_new_content() -> Result<()> {
    let (alice, bob, old_sealed) = grant_flow().await?;
    let bob_id = bob.member_id();

    alice.revoke_member("proj", bob_id).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, None);

    let new_sealed = alice.encrypt("proj", b"post-revocation secret").await?;

    // bob ingests whatever alice still exports to him...
    let events = alice.export_events_for(bob_id).await?;
    let _ = bob.ingest_events(&events).await; // post-revocation exports may be partial

    // ...but the new epoch's key never reaches him.
    assert_eq!(
        bob.decrypt("proj", &new_sealed).await,
        Err(DecryptError::KeyNotFound)
    );
    // content from his member epoch stays readable — by design.
    assert_eq!(
        bob.decrypt("proj", &old_sealed).await.unwrap(),
        b"secret plan"
    );
    // alice still reads everything.
    assert_eq!(
        alice.decrypt("proj", &new_sealed).await.unwrap(),
        b"post-revocation secret"
    );
    Ok(())
}

/// set_role walks bob None -> Read -> Edit -> Read (spec §3.4).
///
/// Assertion strategy for the rotation claims, all via encrypt/decrypt
/// round-trips (the crate's only epoch observable):
/// - upgrade does NOT rotate: content encrypted BEFORE the upgrade still
///   decrypts for bob afterwards — his key continuity survives the
///   revoke + re-grant leg (no eager PCS update).
/// - downgrade DOES rotate: dave, a second member kept current up to the
///   moment before the downgrade, gets KeyNotFound on content encrypted
///   after it — the key state demonstrably advanced past what he holds —
///   while re-granted bob (and dave, once he ingests the rotation ops)
///   decrypts that same content: the downgrade re-keyed bob into the fresh
///   epoch as a reader.
#[tokio::test(flavor = "multi_thread")]
async fn upgrade_then_downgrade() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let (_, bob_auth) = identities(dir.path(), "bob");
    let (_, dave_auth) = identities(dir.path(), "dave");
    let alice = ProjectAuth::new(&alice_auth).await?;
    let bob = ProjectAuth::new(&bob_auth).await?;
    let dave = ProjectAuth::new(&dave_auth).await?;

    alice.create_project("proj").await?;
    let bob_id = alice
        .receive_contact_card(&bob.contact_card().await?)
        .await?;
    let dave_id = alice
        .receive_contact_card(&dave.contact_card().await?)
        .await?;

    // None -> Read: no delegation yet, so set_role is a plain grant.
    assert_eq!(alice.query_access("proj", bob_id).await?, None);
    alice.set_role("proj", bob_id, Role::Read).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, Some(Role::Read));
    // same role again: a no-op, not a stacked second delegation.
    alice.set_role("proj", bob_id, Role::Read).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, Some(Role::Read));

    // dave is the epoch probe for the downgrade leg below.
    alice.add_member("proj", dave_id, Role::Read).await?;

    // encrypt AFTER the grants (#136) so both members can read c1.
    let c1 = alice.encrypt("proj", b"epoch one").await?;
    for (peer, id) in [(&bob, bob_id), (&dave, dave_id)] {
        peer.ingest_events(&alice.export_events_for(id).await?)
            .await?;
        peer.adopt_project("proj", alice.doc_id("proj").unwrap(), None)
            .await?;
        assert_eq!(peer.decrypt("proj", &c1).await.unwrap(), b"epoch one");
    }

    // Read -> Edit (upgrade): no eager rotation — pre-upgrade content still
    // decrypts for bob.
    alice.set_role("proj", bob_id, Role::Edit).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, Some(Role::Edit));
    // post-transition exports may be partial (revocations in the history).
    let _ = bob
        .ingest_events(&alice.export_events_for(bob_id).await?)
        .await;
    assert_eq!(bob.decrypt("proj", &c1).await.unwrap(), b"epoch one");

    // Settle the post-upgrade tree (this encrypt mints any CGKA update the
    // upgrade's revoke + re-add deferred) and bring both members current, so
    // the downgrade below is the ONLY key-state change dave hasn't seen.
    let c2 = alice.encrypt("proj", b"epoch two").await?;
    for (peer, id) in [(&bob, bob_id), (&dave, dave_id)] {
        let _ = peer
            .ingest_events(&alice.export_events_for(id).await?)
            .await;
        assert_eq!(peer.decrypt("proj", &c2).await.unwrap(), b"epoch two");
    }

    // Edit -> Read (downgrade): revoke + eager PCS rotation + re-grant.
    alice.set_role("proj", bob_id, Role::Read).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, Some(Role::Read));

    let c3 = alice.encrypt("proj", b"epoch three").await?;
    // key state advanced: dave, current as of just before the downgrade,
    // cannot derive the post-rotation key from what he already holds...
    assert_eq!(
        dave.decrypt("proj", &c3).await,
        Err(DecryptError::KeyNotFound)
    );
    // ...and it is the key that moved, not dave: c2 still decrypts.
    assert_eq!(dave.decrypt("proj", &c2).await.unwrap(), b"epoch two");
    // re-granted bob reads the fresh epoch once the rotation ops reach him;
    // so does dave — the rotation included the remaining members.
    for (peer, id) in [(&bob, bob_id), (&dave, dave_id)] {
        let _ = peer
            .ingest_events(&alice.export_events_for(id).await?)
            .await;
        assert_eq!(peer.decrypt("proj", &c3).await.unwrap(), b"epoch three");
    }
    Ok(())
}

/// State written by load_or_new survives a drop: member id, membership, and
/// decryption of pre-restart ciphertext (keys ride the archived CGKA tree).
#[tokio::test(flavor = "multi_thread")]
async fn persistence_roundtrip() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let (_, bob_auth) = identities(dir.path(), "bob");
    let state_dir = dir.path().join("alice-state");
    let bob_state_dir = dir.path().join("bob-state");

    let (member_id, bob_id, sealed) = {
        let alice = ProjectAuth::load_or_new(&alice_auth, &state_dir).await?;
        let bob = ProjectAuth::load_or_new(&bob_auth, &bob_state_dir).await?;
        alice.create_project("proj").await?;
        let bob_id = alice
            .receive_contact_card(&bob.contact_card().await?)
            .await?;
        alice.add_member("proj", bob_id, Role::Edit).await?;
        let sealed = alice.encrypt("proj", b"durable secret").await?;
        bob.ingest_events(&alice.export_events_for(bob_id).await?)
            .await?;
        bob.adopt_project(
            "proj",
            alice.doc_id("proj").unwrap(),
            Some(alice.group_id("proj").unwrap()),
        )
        .await?;
        assert_eq!(
            bob.decrypt("proj", &sealed).await.unwrap(),
            b"durable secret"
        );
        (alice.member_id(), bob_id, sealed)
    };

    // invitee side: the adopted project's group id survives a restart.
    {
        let bob = ProjectAuth::load_or_new(&bob_auth, &bob_state_dir).await?;
        assert!(bob.group_id("proj").is_some());
    }

    let alice = ProjectAuth::load_or_new(&alice_auth, &state_dir).await?;
    assert_eq!(alice.member_id(), member_id);
    assert_eq!(alice.query_access("proj", bob_id).await?, Some(Role::Edit));
    assert_eq!(
        alice.decrypt("proj", &sealed).await.unwrap(),
        b"durable secret"
    );

    // revocation and the PCS key rotation survive a restart.
    alice.revoke_member("proj", bob_id).await?;
    drop(alice);
    let alice = ProjectAuth::load_or_new(&alice_auth, &state_dir).await?;
    assert_eq!(alice.query_access("proj", bob_id).await?, None);
    let new_sealed = alice.encrypt("proj", b"post-restart secret").await?;
    let bob = ProjectAuth::load_or_new(&bob_auth, &bob_state_dir).await?;
    let _ = bob
        .ingest_events(&alice.export_events_for(bob_id).await?)
        .await; // post-revocation exports may be partial
    assert_eq!(
        bob.decrypt("proj", &new_sealed).await,
        Err(DecryptError::KeyNotFound)
    );

    std::fs::write(state_dir.join("state.bin"), b"garbage")?;
    assert!(
        ProjectAuth::load_or_new(&alice_auth, &state_dir)
            .await
            .is_err()
    );
    Ok(())
}

/// An invitee who adopted with `bob_role` grants carol Read on its own, and
/// the host learns the new member by ingesting the invitee's events.
async fn invitee_delegates(bob_role: Role) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let (_, bob_auth) = identities(dir.path(), "bob");
    let (_, carol_auth) = identities(dir.path(), "carol");
    let alice = ProjectAuth::new(&alice_auth).await?;
    let bob = ProjectAuth::new(&bob_auth).await?;
    let carol = ProjectAuth::new(&carol_auth).await?;

    alice.create_project("proj").await?;
    let bob_id = alice
        .receive_contact_card(&bob.contact_card().await?)
        .await?;
    alice.add_member("proj", bob_id, bob_role).await?;
    bob.ingest_events(&alice.export_events_for(bob_id).await?)
        .await?;
    bob.adopt_project(
        "proj",
        alice.doc_id("proj").unwrap(),
        Some(alice.group_id("proj").unwrap()),
    )
    .await?;

    // bob grants carol without alice in the loop.
    let carol_id = bob
        .receive_contact_card(&carol.contact_card().await?)
        .await?;
    bob.add_member("proj", carol_id, Role::Read).await?;
    assert_eq!(bob.query_access("proj", carol_id).await?, Some(Role::Read));

    // alice sees carol after learning carol's identity (contact cards travel
    // in the sync preamble) and ingesting bob's events.
    alice
        .receive_contact_card(&carol.contact_card().await?)
        .await?;
    let alice_id = bob
        .receive_contact_card(&alice.contact_card().await?)
        .await?;
    alice
        .ingest_events(&bob.export_events_for(alice_id).await?)
        .await?;
    assert_eq!(
        alice.query_access("proj", carol_id).await?,
        Some(Role::Read)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invitee_manages_membership() -> Result<()> {
    invitee_delegates(Role::Admin).await
}

/// keyhive lets a non-admin (Edit) invitee delegate an attenuated role: the
/// grant signs locally and other members accept it after ingesting.
#[tokio::test(flavor = "multi_thread")]
async fn edit_invitee_add_member() -> Result<()> {
    invitee_delegates(Role::Edit).await
}

/// An Edit invitee delegating a higher role than its own errors (keyhive
/// AccessEscalation).
#[tokio::test(flavor = "multi_thread")]
async fn edit_invitee_cannot_escalate() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let (_, bob_auth) = identities(dir.path(), "bob");
    let (_, carol_auth) = identities(dir.path(), "carol");
    let alice = ProjectAuth::new(&alice_auth).await?;
    let bob = ProjectAuth::new(&bob_auth).await?;
    let carol = ProjectAuth::new(&carol_auth).await?;

    alice.create_project("proj").await?;
    let bob_id = alice
        .receive_contact_card(&bob.contact_card().await?)
        .await?;
    alice.add_member("proj", bob_id, Role::Edit).await?;
    bob.ingest_events(&alice.export_events_for(bob_id).await?)
        .await?;
    bob.adopt_project(
        "proj",
        alice.doc_id("proj").unwrap(),
        Some(alice.group_id("proj").unwrap()),
    )
    .await?;

    let carol_id = bob
        .receive_contact_card(&carol.contact_card().await?)
        .await?;
    assert!(
        bob.add_member("proj", carol_id, Role::Admin).await.is_err(),
        "Edit invitee must not delegate Admin"
    );
    assert_eq!(bob.query_access("proj", carol_id).await?, None);
    Ok(())
}

/// Adopting without the group id: the device can decrypt but does not manage
/// membership, so add_member errors.
#[tokio::test(flavor = "multi_thread")]
async fn adopt_without_group_cannot_manage_membership() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let (_, bob_auth) = identities(dir.path(), "bob");
    let (_, carol_auth) = identities(dir.path(), "carol");
    let alice = ProjectAuth::new(&alice_auth).await?;
    let bob = ProjectAuth::new(&bob_auth).await?;
    let carol = ProjectAuth::new(&carol_auth).await?;

    alice.create_project("proj").await?;
    let bob_id = alice
        .receive_contact_card(&bob.contact_card().await?)
        .await?;
    alice.add_member("proj", bob_id, Role::Edit).await?;
    let sealed = alice.encrypt("proj", b"secret plan").await?;
    bob.ingest_events(&alice.export_events_for(bob_id).await?)
        .await?;
    bob.adopt_project("proj", alice.doc_id("proj").unwrap(), None)
        .await?;

    let carol_id = bob
        .receive_contact_card(&carol.contact_card().await?)
        .await?;
    assert!(
        bob.add_member("proj", carol_id, Role::Read).await.is_err(),
        "no group id -> membership not managed"
    );
    assert_eq!(bob.decrypt("proj", &sealed).await.unwrap(), b"secret plan");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (host_device, host_auth) = identities(dir.path(), "host");
    let (member_device, member_auth) = identities(dir.path(), "member");
    let (stranger_device, stranger_auth) = identities(dir.path(), "stranger");

    // host's capability state: member granted Read, stranger known but ungranted.
    let host_ca = ProjectAuth::new(&host_auth).await?;
    host_ca.create_project("proj").await?;
    let member_ca = ProjectAuth::new(&member_auth).await?;
    let stranger_ca = ProjectAuth::new(&stranger_auth).await?;
    let member_id = host_ca
        .receive_contact_card(&member_ca.contact_card().await?)
        .await?;
    let stranger_id = host_ca
        .receive_contact_card(&stranger_ca.contact_card().await?)
        .await?;
    host_ca.add_member("proj", member_id, Role::Read).await?;

    // keyhive-level: no delegation -> no access.
    assert_eq!(host_ca.query_access("proj", stranger_id).await?, None);

    // callback built from membership + verified bindings.
    let member_binding = DeviceBinding::create(&member_device, &member_auth);
    let stranger_binding = DeviceBinding::create(&stranger_device, &stranger_auth);
    let check = host_ca
        .access_callback(&[member_binding, stranger_binding])
        .await;
    assert!(check(member_device.endpoint_id(), "proj"));
    assert!(!check(stranger_device.endpoint_id(), "proj"));
    assert!(!check(host_device.endpoint_id(), "proj")); // no binding -> deny
    assert!(!check(member_device.endpoint_id(), "other-proj"));

    // end-to-end over real iroh streams: denied peer's sync stream is rejected.
    let host = ShareNode::bind_local(&host_device)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    host.set_access_check(check);
    let mut doc = Automerge::new();
    doc.transact(|tx| tx.put(ROOT, "title", "guarded"))
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    host.register("proj", doc);
    let ticket = host.ticket("proj").map_err(|e| anyhow::anyhow!("{e}"))?;

    let stranger = ShareNode::bind_local(&stranger_device)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(
        stranger.join(&ticket).await.is_err(),
        "stranger's sync must be rejected"
    );

    let member = ShareNode::bind_local(&member_device)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    member
        .join(&ticket)
        .await
        .map_err(|e| anyhow::anyhow!("allow-listed member failed to sync: {e}"))?;
    assert!(member.doc("proj").is_some());

    for node in [host, member, stranger] {
        node.shutdown().await.map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
}

// --- encrypted key store at rest (write-enforcement spec §8) -----------------

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// state.bin persisted with a DEK leaks neither the member key nor a doc id,
/// round-trips with the right DEK, and fails typed with a wrong/missing one.
#[tokio::test(flavor = "multi_thread")]
async fn encrypted_roundtrip() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let state_dir = dir.path().join("alice-state");
    let dek = [7u8; 32];

    let (member_id, doc_id, sealed) = {
        let alice = ProjectAuth::load_or_new_with_dek(&alice_auth, &state_dir, Some(&dek)).await?;
        alice.create_project("proj").await?;
        let sealed = alice.encrypt("proj", b"at-rest secret").await?;
        (alice.member_id(), alice.doc_id("proj").unwrap(), sealed)
    };

    let raw = std::fs::read(state_dir.join("state.bin"))?;
    assert_eq!(raw[0], 2, "state persisted with a DEK must be format v2");
    // neither the member signing key (seed or verifying half) nor a known
    // project doc id appears in the clear.
    let seed = std::fs::read(dir.path().join("alice.keyhive.key"))?;
    assert!(!contains(&raw, &seed), "signing seed in the clear");
    assert!(!contains(&raw, &member_id.0), "member key in the clear");
    assert!(!contains(&raw, &doc_id), "doc id in the clear");

    // the same DEK recovers identity, registry, and key material.
    let alice = ProjectAuth::load_or_new_with_dek(&alice_auth, &state_dir, Some(&dek)).await?;
    assert_eq!(alice.member_id(), member_id);
    assert_eq!(alice.doc_id("proj"), Some(doc_id));
    assert_eq!(
        alice.decrypt("proj", &sealed).await.unwrap(),
        b"at-rest secret"
    );

    // wrong DEK: typed error, not a panic or decode error.
    let err = ProjectAuth::load_or_new_with_dek(&alice_auth, &state_dir, Some(&[8u8; 32]))
        .await
        .unwrap_err();
    assert_eq!(
        err.downcast_ref::<KeyStoreError>(),
        Some(&KeyStoreError::WrongDek)
    );

    // no DEK at all: locked store.
    let err = ProjectAuth::load_or_new(&alice_auth, &state_dir)
        .await
        .unwrap_err();
    assert_eq!(
        err.downcast_ref::<KeyStoreError>(),
        Some(&KeyStoreError::Locked)
    );
    Ok(())
}

/// v1 plaintext state loaded with a DEK is re-persisted encrypted (v2, no
/// plaintext left behind) with the identity and registry unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn state_migrates_v1_to_encrypted() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth) = identities(dir.path(), "alice");
    let state_dir = dir.path().join("alice-state");

    let (member_id, doc_id) = {
        let alice = ProjectAuth::load_or_new(&alice_auth, &state_dir).await?;
        alice.create_project("proj").await?;
        (alice.member_id(), alice.doc_id("proj").unwrap())
    };
    let raw = std::fs::read(state_dir.join("state.bin"))?;
    assert_eq!(raw[0], 1);
    assert!(
        contains(&raw, &doc_id),
        "v1 plaintext registry carries the doc id"
    );

    let dek = [9u8; 32];
    let alice = ProjectAuth::load_or_new_with_dek(&alice_auth, &state_dir, Some(&dek)).await?;
    assert_eq!(alice.member_id(), member_id);
    assert_eq!(alice.doc_id("proj"), Some(doc_id));
    drop(alice);

    let raw = std::fs::read(state_dir.join("state.bin"))?;
    assert_eq!(raw[0], 2, "v1 must be re-persisted encrypted on load");
    assert!(!contains(&raw, &doc_id), "plaintext gone after migration");

    let alice = ProjectAuth::load_or_new_with_dek(&alice_auth, &state_dir, Some(&dek)).await?;
    assert_eq!(alice.member_id(), member_id);
    assert_eq!(alice.doc_id("proj"), Some(doc_id));
    Ok(())
}

fn io_key_store_err(err: &std::io::Error) -> Option<&KeyStoreError> {
    err.get_ref()
        .and_then(|e| e.downcast_ref::<KeyStoreError>())
}

/// device.key generated with a DEK is not the bare seed, round-trips with the
/// right DEK, and fails typed with a wrong/missing one.
#[test]
fn device_key_encrypted_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.key");
    let dek = [7u8; 32];

    let first = DeviceIdentity::load_or_generate_with_dek(&path, Some(&dek)).unwrap();
    let raw = std::fs::read(&path).unwrap();
    assert_ne!(raw.len(), 32, "file must not be the bare 32-byte seed");

    let second = DeviceIdentity::load_or_generate_with_dek(&path, Some(&dek)).unwrap();
    assert_eq!(first.endpoint_id(), second.endpoint_id());

    let err = DeviceIdentity::load_or_generate_with_dek(&path, Some(&[8u8; 32])).unwrap_err();
    assert_eq!(io_key_store_err(&err), Some(&KeyStoreError::WrongDek));

    // the DEK-less legacy loader sees a locked store, not garbage.
    let err = DeviceIdentity::load_or_generate(&path).unwrap_err();
    assert_eq!(io_key_store_err(&err), Some(&KeyStoreError::Locked));
}

/// A legacy plaintext device.key is rewritten encrypted once a DEK shows up:
/// seed bytes gone from disk, identity unchanged.
#[test]
fn device_key_migrates_to_encrypted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.key");

    let first = DeviceIdentity::load_or_generate(&path).unwrap();
    let seed = std::fs::read(&path).unwrap();
    assert_eq!(seed.len(), 32);

    let dek = [7u8; 32];
    let migrated = DeviceIdentity::load_or_generate_with_dek(&path, Some(&dek)).unwrap();
    assert_eq!(migrated.endpoint_id(), first.endpoint_id());

    let raw = std::fs::read(&path).unwrap();
    assert!(!contains(&raw, &seed), "seed must not remain in the clear");

    let again = DeviceIdentity::load_or_generate_with_dek(&path, Some(&dek)).unwrap();
    assert_eq!(again.endpoint_id(), first.endpoint_id());
}

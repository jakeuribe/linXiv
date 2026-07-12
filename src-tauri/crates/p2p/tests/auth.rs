#![cfg(feature = "auth-keyhive")]

use anyhow::Result;
use automerge::{Automerge, ROOT, transaction::Transactable};
use linxiv_p2p::{
    DeviceIdentity, ShareNode,
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
/// ships an "invite" (endpoint id + doc id + delegation events); bob ingests,
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

    // an invite = hoster endpoint + project/doc ids + delegation events.
    let invite = postcard::to_stdvec(&(
        *alice_device.endpoint_id().as_bytes(),
        "proj",
        alice.doc_id("proj").unwrap(),
        alice.export_events_for(bob_id).await?,
    ))?;

    let (_host, project_id, doc_id, events): ([u8; 32], String, [u8; 32], Vec<u8>) =
        postcard::from_bytes(&invite)?;
    bob.ingest_events(&events).await?; // Err if any events are stuck
    bob.adopt_project(&project_id, doc_id).await?;

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

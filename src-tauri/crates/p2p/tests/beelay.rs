#![cfg(feature = "sync-beelay")]

use anyhow::Result;
use automerge::{Automerge, ROOT, ReadDoc, transaction::Transactable};
use linxiv_p2p::{
    BeelayNode, DeviceIdentity,
    auth::{AuthIdentity, DecryptError, DeviceBinding, ProjectAuth, Role},
    bind_stack_local,
};

fn identities(dir: &std::path::Path, name: &str) -> (DeviceIdentity, AuthIdentity) {
    let device = DeviceIdentity::load_or_generate(dir.join(format!("{name}.iroh.key"))).unwrap();
    let auth = AuthIdentity::load_or_generate(dir.join(format!("{name}.keyhive.key"))).unwrap();
    (device, auth)
}

fn get_str(doc: &Automerge, key: &str) -> Option<String> {
    doc.get(ROOT, key)
        .unwrap()
        .and_then(|(v, _)| v.into_string().ok())
}

fn put(doc: &mut Automerge, key: &str, value: &str) {
    doc.transact(|tx| tx.put(ROOT, key, value))
        .map(|_| ())
        .unwrap();
}

/// THE PLAN GATE: two offline nodes; alice shares an encrypted project with
/// bob, both edit and converge, then alice revokes bob and rotates — bob
/// keeps his epoch's content but is refused further sync.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_toy_project() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth_id) = identities(dir.path(), "alice");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let alice_auth = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;

    // contact-card exchange (out of band), then the nodes
    let bob_member = alice_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let alice = BeelayNode::bind_local(&alice_device, &alice_auth_id, alice_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;

    // alice creates the project with initial content, grants bob Edit, invites
    let mut doc = Automerge::new();
    put(&mut doc, "title", "Toy Project");
    alice
        .create_shared_project("proj", doc)
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .auth()
        .add_member("proj", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    let invite = alice
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;

    // bob accepts (pasteable string) and syncs -> decrypts the content
    let project_id = bob
        .accept_invite(&invite)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(project_id, "proj");
    let outcome = bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    assert!(
        outcome.undecryptable.is_empty(),
        "{:?}",
        outcome.undecryptable
    );
    assert!(outcome.applied >= 1);
    let bob_doc = bob.doc("proj").await.expect("bob has the project");
    assert_eq!(get_str(&bob_doc, "title").as_deref(), Some("Toy Project"));

    // both sides edit, bob re-syncs, both converge
    alice
        .with_doc("proj", |d| put(d, "alice_edit", "from alice"))
        .await;
    bob.with_doc("proj", |d| put(d, "bob_edit", "from bob"))
        .await;
    let outcome = bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    assert!(
        outcome.undecryptable.is_empty(),
        "{:?}",
        outcome.undecryptable
    );
    for (name, node) in [("alice", &alice), ("bob", &bob)] {
        let doc = node.doc("proj").await.unwrap();
        assert_eq!(
            get_str(&doc, "title").as_deref(),
            Some("Toy Project"),
            "{name}"
        );
        assert_eq!(
            get_str(&doc, "alice_edit").as_deref(),
            Some("from alice"),
            "{name}"
        );
        assert_eq!(
            get_str(&doc, "bob_edit").as_deref(),
            Some("from bob"),
            "{name}"
        );
    }

    // alice revokes bob (revoke_member also rotates the doc key) + confirms
    alice
        .auth()
        .revoke_member("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(
        alice
            .auth()
            .query_access("proj", bob_member)
            .await
            .map_err(anyhow::Error::msg)?,
        None
    );

    // alice adds new content, encrypted at the rotated epoch
    alice
        .with_doc("proj", |d| put(d, "post_revocation", "secret v2"))
        .await;

    // bob syncs again: refused at the door by the accept-side membership gate
    let res = bob.sync_project("proj").await;
    assert!(
        matches!(res, Err(linxiv_p2p::sync::JoinError::Refused)),
        "{res:?}"
    );
    let bob_doc = bob.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&bob_doc, "post_revocation"),
        None,
        "revoked bob read new content"
    );
    // his old epoch's content stays readable — by design
    assert_eq!(get_str(&bob_doc, "title").as_deref(), Some("Toy Project"));
    assert_eq!(
        get_str(&bob_doc, "alice_edit").as_deref(),
        Some("from alice")
    );
    // alice keeps everything
    let alice_doc = alice.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&alice_doc, "post_revocation").as_deref(),
        Some("secret v2")
    );

    for node in [alice, bob] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// Upstream keyhive #136 at our stack level
/// (https://github.com/inkandswitch/keyhive/issues/136): content encrypted
/// BEFORE a member is granted stays undecryptable to them — the initial-
/// commit sync failure shape. The carried workaround: re-encrypt after the
/// grant (BeelayNode bakes this in by flushing lazily at invite/sync time).
#[tokio::test(flavor = "multi_thread")]
async fn keyhive_136_repro_and_workaround() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (_, alice_auth_id) = identities(dir.path(), "alice");
    let (_, bob_auth_id) = identities(dir.path(), "bob");
    let alice = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;

    alice
        .create_project("proj")
        .await
        .map_err(anyhow::Error::msg)?;
    // ENCRYPT BEFORE GRANT — the #136 trap
    let early = alice
        .encrypt("proj", b"initial content")
        .await
        .map_err(anyhow::Error::msg)?;

    let bob_id = alice
        .receive_contact_card(&bob.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .add_member("proj", bob_id, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    // "sync": ship alice's events to bob, bob adopts the doc
    bob.ingest_events(
        &alice
            .export_events_for(bob_id)
            .await
            .map_err(anyhow::Error::msg)?,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    bob.adopt_project("proj", alice.doc_id("proj").unwrap(), None)
        .await
        .map_err(anyhow::Error::msg)?;

    // #136 failure shape: pre-grant ciphertext -> KeyNotFound for the new member
    assert_eq!(
        bob.decrypt("proj", &early).await,
        Err(DecryptError::KeyNotFound),
        "keyhive #136 no longer reproduces — re-evaluate grant-before-encrypt ordering"
    );

    // workaround: alice re-encrypts the same content AFTER the grant
    let reencrypted = alice
        .encrypt("proj", b"initial content")
        .await
        .map_err(anyhow::Error::msg)?;
    // encryption can mint fresh CGKA ops — ship events again
    bob.ingest_events(
        &alice
            .export_events_for(bob_id)
            .await
            .map_err(anyhow::Error::msg)?,
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert_eq!(
        bob.decrypt("proj", &reencrypted).await.unwrap(),
        b"initial content"
    );
    Ok(())
}

/// Two loopback nodes with a shared project: alice grants bob, stores the
/// blobs (encrypt-after-grant), then bob joins via invite. Returns the nodes
/// and the tickets, ready for bob to fetch.
async fn blob_pair(sizes: &[usize]) -> Result<(BeelayNode, BeelayNode, Vec<(Vec<u8>, String)>)> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth_id) = identities(dir.path(), "alice");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let alice_auth = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_member = alice_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let alice = BeelayNode::bind_local(&alice_device, &alice_auth_id, alice_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .create_shared_project("proj", Automerge::new())
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .auth()
        .add_member("proj", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;

    let mut blobs = Vec::new();
    for &size in sizes {
        let bytes: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let start = std::time::Instant::now();
        let ticket = alice
            .store_blob("proj", &bytes)
            .await
            .map_err(anyhow::Error::msg)?;
        print_rate("encrypt+store", size, start.elapsed());
        blobs.push((bytes, ticket));
    }
    // invite AFTER storing so bob's events cover the blobs' epoch
    let invite = alice
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.accept_invite(&invite)
        .await
        .map_err(anyhow::Error::msg)?;
    // one sync so alice's preamble learns bob's device binding — the blobs
    // ALPN gate only serves endpoints it can map to a member
    bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    Ok((alice, bob, blobs))
}

fn print_rate(phase: &str, size: usize, elapsed: std::time::Duration) {
    let mib = size as f64 / (1024.0 * 1024.0);
    println!(
        "[{mib:>5.1} MiB] {phase:>13}: {:>7.1} ms ({:>7.1} MiB/s)",
        elapsed.as_secs_f64() * 1000.0,
        mib / elapsed.as_secs_f64()
    );
}

/// Bob fetches each blob over loopback iroh, decrypts, and gets alice's
/// original bytes back; prints per-phase throughput.
async fn blob_round_trip(sizes: &[usize]) -> Result<()> {
    let (alice, bob, blobs) = blob_pair(sizes).await?;
    for (bytes, ticket) in &blobs {
        let start = std::time::Instant::now();
        bob.fetch_blob(ticket, u64::MAX)
            .await
            .map_err(anyhow::Error::msg)?;
        print_rate("transfer", bytes.len(), start.elapsed());
        let start = std::time::Instant::now();
        let plain = bob
            .read_blob("proj", ticket, u64::MAX)
            .await
            .map_err(anyhow::Error::msg)?;
        print_rate("decrypt+read", bytes.len(), start.elapsed());
        assert_eq!(&plain, bytes, "round-tripped blob differs");
    }
    for node in [alice, bob] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// Fast blob gate: a 64 KiB "PDF" round-trips encrypted between two nodes.
#[tokio::test(flavor = "multi_thread")]
async fn blob_round_trip_small() -> Result<()> {
    blob_round_trip(&[64 * 1024]).await
}

/// A cap below the blob size makes fetch_blob and read_blob error; a
/// generous cap succeeds and round-trips the original bytes.
#[tokio::test(flavor = "multi_thread")]
async fn blob_caps_enforced() -> Result<()> {
    let (alice, bob, blobs) = blob_pair(&[64 * 1024]).await?;
    let (bytes, ticket) = &blobs[0];
    assert!(bob.fetch_blob(ticket, 1024).await.is_err());
    bob.fetch_blob(ticket, u64::MAX)
        .await
        .map_err(anyhow::Error::msg)?;
    assert!(bob.read_blob("proj", ticket, 1024).await.is_err());
    let plain = bob
        .read_blob("proj", ticket, u64::MAX)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(&plain, bytes);
    for node in [alice, bob] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// PLAN gate: real linXiv project sizes. Run with
/// `cargo test --features sync-beelay -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "slow: 5 + 25 MiB transfers; run with -- --ignored --nocapture"]
async fn blob_round_trip_project_sizes() -> Result<()> {
    blob_round_trip(&[5 * 1024 * 1024, 25 * 1024 * 1024]).await
}

/// A connection whose beelay hello announces a PeerId different from the
/// TLS-authenticated iroh endpoint id must be dropped before any reply.
#[tokio::test(flavor = "multi_thread")]
async fn peer_id_spoof_rejected() -> Result<()> {
    use iroh::endpoint::presets;

    let dir = tempfile::tempdir()?;
    let (host_device, host_auth_id) = identities(dir.path(), "host");
    let (imposter_device, imposter_auth_id) = identities(dir.path(), "imposter");
    let host_auth = ProjectAuth::new(&host_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let imposter_auth = ProjectAuth::new(&imposter_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let host = BeelayNode::bind_local(&host_device, &host_auth_id, host_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    // grant the imposter membership on "proj" so the accept-side gate passes
    let imposter_member = host
        .auth()
        .receive_contact_card(
            &imposter_auth
                .contact_card()
                .await
                .map_err(anyhow::Error::msg)?,
        )
        .await
        .map_err(anyhow::Error::msg)?;
    host.create_shared_project("proj", Automerge::new())
        .await
        .map_err(anyhow::Error::msg)?;
    host.auth()
        .add_member("proj", imposter_member, Role::Read)
        .await
        .map_err(anyhow::Error::msg)?;

    // raw wire-level client so we control the hello bytes; reuses the
    // imposter's persisted device key so its DeviceBinding matches the
    // connection's TLS endpoint (the preamble verifies exactly that)
    let key_bytes: [u8; 32] = std::fs::read(dir.path().join("imposter.iroh.key"))?
        .as_slice()
        .try_into()
        .expect("device key file is 32 bytes");
    let endpoint = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(iroh::SecretKey::from_bytes(&key_bytes))
        .bind()
        .await?;
    let conn = endpoint
        .connect(host.addr(), linxiv_p2p::BEELAY_ALPN)
        .await?;
    let (mut send, mut recv) = conn.open_bi().await?;

    // frame helpers pinning the wire format: 8-byte LE length, 1 tag byte
    async fn write_frame(
        send: &mut iroh::endpoint::SendStream,
        tag: u8,
        body: &[u8],
    ) -> Result<()> {
        send.write_all(&((body.len() + 1) as u64).to_le_bytes())
            .await?;
        send.write_all(&[tag]).await?;
        send.write_all(body).await?;
        Ok(())
    }
    async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<(u8, Vec<u8>)> {
        let mut len = [0u8; 8];
        recv.read_exact(&mut len).await?;
        let len = u64::from_le_bytes(len) as usize;
        anyhow::ensure!(len > 0, "unexpected empty frame");
        let mut buf = vec![0u8; len];
        recv.read_exact(&mut buf).await?;
        Ok((buf[0], buf.split_off(1)))
    }

    // project-id frame first (plain, untagged), matching sync_project's opener
    send.write_all(&(b"proj".len() as u64).to_le_bytes())
        .await?;
    send.write_all(b"proj").await?;

    // legitimate keyhive preamble (tag 1 both ways, binding+card hellos then
    // events)
    let binding = DeviceBinding::create(&imposter_device, &imposter_auth_id);
    let hello = postcard::to_stdvec(&(
        &binding,
        imposter_auth
            .contact_card()
            .await
            .map_err(anyhow::Error::msg)?,
    ))?;
    write_frame(&mut send, 1, &hello).await?;
    let (tag, host_hello) = read_frame(&mut recv).await?;
    assert_eq!(tag, 1);
    let (_host_binding, card): (DeviceBinding, Vec<u8>) = postcard::from_bytes(&host_hello)?;
    let host_member = imposter_auth
        .receive_contact_card(&card)
        .await
        .map_err(anyhow::Error::msg)?;
    write_frame(
        &mut send,
        1,
        &imposter_auth
            .export_events_for(host_member)
            .await
            .map_err(anyhow::Error::msg)?,
    )
    .await?;
    let (tag, _events) = read_frame(&mut recv).await?;
    assert_eq!(tag, 1);

    // spoofed beelay hello (tag 0): message type 0 = HelloDearServer,
    // uleb128 length + peer id bytes — a PeerId that is NOT our endpoint id.
    let fake_peer = b"imposter-peer-id";
    let mut hello = vec![0u8, fake_peer.len() as u8];
    hello.extend_from_slice(fake_peer);
    write_frame(&mut send, 0, &hello).await?;

    // the host must drop the connection without replying to the hello
    let rejected = read_frame(&mut recv).await;
    assert!(
        rejected.is_err(),
        "host answered a spoofed hello: {rejected:?}"
    );

    host.shutdown().await.map_err(anyhow::Error::msg)?;
    endpoint.close().await;
    Ok(())
}

/// Synced doc content survives a restart from data_dir: bob syncs, restarts
/// with the same dir + reloaded auth, and the doc is present WITHOUT any
/// sync_project call (decrypted straight from the persisted beelay KV).
#[tokio::test(flavor = "multi_thread")]
async fn kv_restart_keeps_doc() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth_id) = identities(dir.path(), "alice");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let alice_auth = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth_dir = dir.path().join("bob-auth");
    let bob_data_dir = dir.path().join("bob-data");
    let bob_auth = ProjectAuth::load_or_new(&bob_auth_id, &bob_auth_dir)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_member = alice_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let alice = BeelayNode::bind_local(&alice_device, &alice_auth_id, alice_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, Some(&bob_data_dir))
        .await
        .map_err(anyhow::Error::msg)?;

    let mut doc = Automerge::new();
    put(&mut doc, "title", "Persist Me");
    alice
        .create_shared_project("proj", doc)
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .auth()
        .add_member("proj", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    let invite = alice
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.accept_invite(&invite)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    let bob_doc = bob.doc("proj").await.expect("bob has the project");
    assert_eq!(get_str(&bob_doc, "title").as_deref(), Some("Persist Me"));
    bob.shutdown().await.map_err(anyhow::Error::msg)?;
    drop(bob);

    // restart: fresh node over the same data_dir + persisted auth
    let bob_auth = ProjectAuth::load_or_new(&bob_auth_id, &bob_auth_dir)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, Some(&bob_data_dir))
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_doc = bob
        .doc("proj")
        .await
        .expect("project registry restored from disk");
    assert_eq!(get_str(&bob_doc, "title").as_deref(), Some("Persist Me"));

    for node in [alice, bob] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// The accept side refuses peers with no role on the requested project: a
/// leaked invite lets a stranger adopt locally, but the host closes the sync
/// before serving; the invited member is unaffected.
#[tokio::test(flavor = "multi_thread")]
async fn stranger_sync_refused() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth_id) = identities(dir.path(), "alice");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let (mallory_device, mallory_auth_id) = identities(dir.path(), "mallory");
    let alice_auth = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let mallory_auth = ProjectAuth::new(&mallory_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_card = bob_auth.contact_card().await.map_err(anyhow::Error::msg)?;
    let bob_member = alice_auth
        .receive_contact_card(&bob_card)
        .await
        .map_err(anyhow::Error::msg)?;
    // mallory knows bob's (public) card too — the invite's events reference it
    mallory_auth
        .receive_contact_card(&bob_card)
        .await
        .map_err(anyhow::Error::msg)?;
    let alice = BeelayNode::bind_local(&alice_device, &alice_auth_id, alice_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let mallory = BeelayNode::bind_local(&mallory_device, &mallory_auth_id, mallory_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;

    let mut doc = Automerge::new();
    put(&mut doc, "title", "Members Only");
    alice
        .create_shared_project("proj", doc)
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .auth()
        .add_member("proj", bob_member, Role::Read)
        .await
        .map_err(anyhow::Error::msg)?;
    let invite = alice
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;

    // mallory got the invite string but was never granted: adopting locally
    // works, the host refuses to serve.
    mallory
        .accept_invite(&invite)
        .await
        .map_err(anyhow::Error::msg)?;
    let res = mallory.sync_project("proj").await;
    assert!(
        matches!(res, Err(linxiv_p2p::sync::JoinError::Refused)),
        "{res:?}"
    );
    let mallory_doc = mallory.doc("proj").await.unwrap();
    assert_eq!(get_str(&mallory_doc, "title"), None);

    // the invited member still syncs
    bob.accept_invite(&invite)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    let bob_doc = bob.doc("proj").await.unwrap();
    assert_eq!(get_str(&bob_doc, "title").as_deref(), Some("Members Only"));

    for node in [alice, bob, mallory] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// Write enforcement (spec §2): a Read-role member syncs serve-only — the
/// session completes and the viewer receives the host's content, but the
/// viewer's uploads land in a throwaway core and never reach the host's
/// canonical store or other members.
#[tokio::test(flavor = "multi_thread")]
async fn viewer_cannot_write() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (hoster_device, hoster_auth_id) = identities(dir.path(), "hoster");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let (carol_device, carol_auth_id) = identities(dir.path(), "carol");
    let hoster_auth = ProjectAuth::new(&hoster_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_auth = ProjectAuth::new(&carol_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_member = hoster_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_member = hoster_auth
        .receive_contact_card(&carol_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let hoster = BeelayNode::bind_local(&hoster_device, &hoster_auth_id, hoster_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol = BeelayNode::bind_local(&carol_device, &carol_auth_id, carol_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;

    let mut doc = Automerge::new();
    put(&mut doc, "title", "Look Don't Touch");
    hoster
        .create_shared_project("proj", doc)
        .await
        .map_err(anyhow::Error::msg)?;
    hoster
        .auth()
        .add_member("proj", bob_member, Role::Read)
        .await
        .map_err(anyhow::Error::msg)?;
    hoster
        .auth()
        .add_member("proj", carol_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_invite = hoster
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_invite = hoster
        .invite("proj", carol_member)
        .await
        .map_err(anyhow::Error::msg)?;

    // bob (Read) pulls the content, edits his mirror, and syncs again: the
    // whole flow must complete — serve-only is not refused — but his upload
    // evaporates with the host's scratch core.
    bob.accept_invite(&bob_invite)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    bob.with_doc("proj", |d| put(d, "bob_edit", "sneaky write"))
        .await;
    let outcome = bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    assert!(
        outcome.undecryptable.is_empty(),
        "{:?}",
        outcome.undecryptable
    );
    // serve-only ≠ refused: bob did receive the hoster's content
    let bob_doc = bob.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&bob_doc, "title").as_deref(),
        Some("Look Don't Touch")
    );

    // carol (Edit) syncs; her session ends with a host refresh, so if bob's
    // commit had reached the canonical store it would surface in BOTH docs.
    carol
        .accept_invite(&carol_invite)
        .await
        .map_err(anyhow::Error::msg)?;
    carol
        .sync_project("proj")
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_doc = carol.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&carol_doc, "title").as_deref(),
        Some("Look Don't Touch")
    );
    assert_eq!(
        get_str(&carol_doc, "bob_edit"),
        None,
        "viewer write propagated to a member"
    );
    let hoster_doc = hoster.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&hoster_doc, "bob_edit"),
        None,
        "viewer write reached the host"
    );

    for node in [hoster, bob, carol] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// Role transitions meet write enforcement (spec §3.4): bob's push lands
/// while he is an Editor, then a set_role downgrade to Read flips his
/// sessions to serve-only — his next push evaporates in the scratch core,
/// a third Editor never sees it, and he still RECEIVES new host content.
#[tokio::test(flavor = "multi_thread")]
async fn downgraded_editor_cannot_write() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (hoster_device, hoster_auth_id) = identities(dir.path(), "hoster");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let (carol_device, carol_auth_id) = identities(dir.path(), "carol");
    let hoster_auth = ProjectAuth::new(&hoster_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_auth = ProjectAuth::new(&carol_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_member = hoster_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_member = hoster_auth
        .receive_contact_card(&carol_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let hoster = BeelayNode::bind_local(&hoster_device, &hoster_auth_id, hoster_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol = BeelayNode::bind_local(&carol_device, &carol_auth_id, carol_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;

    let mut doc = Automerge::new();
    put(&mut doc, "title", "Editors Wanted");
    hoster
        .create_shared_project("proj", doc)
        .await
        .map_err(anyhow::Error::msg)?;
    hoster
        .auth()
        .add_member("proj", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    hoster
        .auth()
        .add_member("proj", carol_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_invite = hoster
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_invite = hoster
        .invite("proj", carol_member)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.accept_invite(&bob_invite)
        .await
        .map_err(anyhow::Error::msg)?;
    carol
        .accept_invite(&carol_invite)
        .await
        .map_err(anyhow::Error::msg)?;

    // bob (Edit) pushes a change: accepted into the canonical store and
    // visible to a third Editor.
    bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    bob.with_doc("proj", |d| put(d, "bob_edit_1", "while editor"))
        .await;
    bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    carol
        .sync_project("proj")
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_doc = carol.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&carol_doc, "bob_edit_1").as_deref(),
        Some("while editor"),
        "editor push must reach other members"
    );

    // downgrade Edit -> Read (revoke + rotate + re-grant), then new host
    // content at the fresh epoch.
    hoster
        .auth()
        .set_role("proj", bob_member, Role::Read)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(
        hoster
            .auth()
            .query_access("proj", bob_member)
            .await
            .map_err(anyhow::Error::msg)?,
        Some(Role::Read)
    );
    hoster
        .with_doc("proj", |d| put(d, "post_downgrade", "fresh"))
        .await;

    // bob pushes again: the session completes (serve-only, not refused) and
    // he receives the host's new content, but his upload evaporates.
    bob.with_doc("proj", |d| put(d, "bob_edit_2", "as viewer"))
        .await;
    let outcome = bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    assert!(
        outcome.undecryptable.is_empty(),
        "{:?}",
        outcome.undecryptable
    );
    let bob_doc = bob.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&bob_doc, "post_downgrade").as_deref(),
        Some("fresh"),
        "downgraded member must still receive host content"
    );

    // the second push never reached the host or the third Editor.
    carol
        .sync_project("proj")
        .await
        .map_err(anyhow::Error::msg)?;
    let carol_doc = carol.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&carol_doc, "post_downgrade").as_deref(),
        Some("fresh")
    );
    assert_eq!(
        get_str(&carol_doc, "bob_edit_2"),
        None,
        "downgraded member's write propagated to a member"
    );
    let hoster_doc = hoster.doc("proj").await.unwrap();
    assert_eq!(
        get_str(&hoster_doc, "bob_edit_2"),
        None,
        "downgraded member's write reached the host"
    );

    for node in [hoster, bob, carol] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

/// One endpoint serves plain sync, beelay, and blobs: both handles of a
/// stack report the same endpoint id, both protocols work between the same
/// two stacks, and shutdown is exercised from either handle in either order.
#[tokio::test(flavor = "multi_thread")]
async fn bind_stack_single_endpoint() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth_id) = identities(dir.path(), "alice");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let alice_auth = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_member = alice_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let (alice_share, alice_beelay) =
        bind_stack_local(&alice_device, &alice_auth_id, alice_auth, None)
            .await
            .map_err(anyhow::Error::msg)?;
    let (bob_share, bob_beelay) = bind_stack_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(alice_share.endpoint_id(), alice_beelay.endpoint_id());
    assert_eq!(bob_share.endpoint_id(), bob_beelay.endpoint_id());

    // beelay path
    let mut doc = Automerge::new();
    put(&mut doc, "title", "Stacked");
    alice_beelay
        .create_shared_project("proj", doc)
        .await
        .map_err(anyhow::Error::msg)?;
    alice_beelay
        .auth()
        .add_member("proj", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    let invite = alice_beelay
        .invite("proj", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    bob_beelay
        .accept_invite(&invite)
        .await
        .map_err(anyhow::Error::msg)?;
    bob_beelay
        .sync_project("proj")
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_doc = bob_beelay.doc("proj").await.unwrap();
    assert_eq!(get_str(&bob_doc, "title").as_deref(), Some("Stacked"));

    // plain sync path over the SAME endpoints
    let mut plain = Automerge::new();
    put(&mut plain, "title", "Plain");
    alice_share.register("plain", plain);
    let ticket = alice_share.ticket("plain").map_err(anyhow::Error::msg)?;
    bob_share
        .join(&ticket)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let plain_doc = bob_share.doc("plain").unwrap();
    assert_eq!(get_str(&plain_doc, "title").as_deref(), Some("Plain"));

    // shutdown both ways: share-then-beelay and beelay-then-share
    alice_share.shutdown().await.map_err(anyhow::Error::msg)?;
    alice_beelay.shutdown().await.map_err(anyhow::Error::msg)?;
    bob_beelay.shutdown().await.map_err(anyhow::Error::msg)?;
    bob_share.shutdown().await.map_err(anyhow::Error::msg)?;
    Ok(())
}

/// A stored blob survives a restart from data_dir: store, restart with the
/// same dir + reloaded auth, read_blob still returns the plaintext.
#[tokio::test(flavor = "multi_thread")]
async fn blob_restart_keeps_blob() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (device, auth_id) = identities(dir.path(), "host");
    let auth_dir = dir.path().join("auth");
    let data_dir = dir.path().join("data");
    let auth = ProjectAuth::load_or_new(&auth_id, &auth_dir)
        .await
        .map_err(anyhow::Error::msg)?;
    let node = BeelayNode::bind_local(&device, &auth_id, auth, Some(&data_dir))
        .await
        .map_err(anyhow::Error::msg)?;
    node.create_shared_project("proj", Automerge::new())
        .await
        .map_err(anyhow::Error::msg)?;
    let bytes: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let ticket = node
        .store_blob("proj", &bytes)
        .await
        .map_err(anyhow::Error::msg)?;
    node.shutdown().await.map_err(anyhow::Error::msg)?;
    drop(node);

    let auth = ProjectAuth::load_or_new(&auth_id, &auth_dir)
        .await
        .map_err(anyhow::Error::msg)?;
    let node = BeelayNode::bind_local(&device, &auth_id, auth, Some(&data_dir))
        .await
        .map_err(anyhow::Error::msg)?;
    let plain = node
        .read_blob("proj", &ticket, u64::MAX)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(plain, bytes);
    node.shutdown().await.map_err(anyhow::Error::msg)?;
    Ok(())
}

/// Revocation is scoped per project: bob loses "a" but keeps his membership
/// on "b", so "a" refuses his sync while "b" still serves him.
#[tokio::test(flavor = "multi_thread")]
async fn revocation_is_per_project() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let (alice_device, alice_auth_id) = identities(dir.path(), "alice");
    let (bob_device, bob_auth_id) = identities(dir.path(), "bob");
    let alice_auth = ProjectAuth::new(&alice_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_auth = ProjectAuth::new(&bob_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob_member = alice_auth
        .receive_contact_card(&bob_auth.contact_card().await.map_err(anyhow::Error::msg)?)
        .await
        .map_err(anyhow::Error::msg)?;
    let alice = BeelayNode::bind_local(&alice_device, &alice_auth_id, alice_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, &bob_auth_id, bob_auth, None)
        .await
        .map_err(anyhow::Error::msg)?;

    alice
        .create_shared_project("a", Automerge::new())
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .create_shared_project("b", Automerge::new())
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .auth()
        .add_member("a", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    alice
        .auth()
        .add_member("b", bob_member, Role::Edit)
        .await
        .map_err(anyhow::Error::msg)?;
    let invite_a = alice
        .invite("a", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    let invite_b = alice
        .invite("b", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.accept_invite(&invite_a)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.accept_invite(&invite_b)
        .await
        .map_err(anyhow::Error::msg)?;
    bob.sync_project("a").await.map_err(anyhow::Error::msg)?;
    bob.sync_project("b").await.map_err(anyhow::Error::msg)?;

    alice
        .auth()
        .revoke_member("a", bob_member)
        .await
        .map_err(anyhow::Error::msg)?;
    alice.with_doc("b", |d| put(d, "still_shared", "yes")).await;

    let res = bob.sync_project("a").await;
    assert!(
        matches!(res, Err(linxiv_p2p::sync::JoinError::Refused)),
        "{res:?}"
    );
    bob.sync_project("b").await.map_err(anyhow::Error::msg)?;
    let bob_doc_b = bob.doc("b").await.unwrap();
    assert_eq!(get_str(&bob_doc_b, "still_shared").as_deref(), Some("yes"));

    for node in [alice, bob] {
        node.shutdown().await.map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

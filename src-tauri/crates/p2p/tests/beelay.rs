#![cfg(feature = "sync-beelay")]

use anyhow::Result;
use automerge::{Automerge, ROOT, ReadDoc, transaction::Transactable};
use linxiv_p2p::{
    BeelayNode, DeviceIdentity,
    auth::{AuthIdentity, DecryptError, ProjectAuth, Role},
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
/// bob, both edit and converge, then alice revokes bob and rotates — bob can
/// still read his epoch's content but nothing new.
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
    let alice = BeelayNode::bind_local(&alice_device, alice_auth)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, bob_auth)
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

    // bob syncs again: fetches ciphertext but CANNOT decrypt the new epoch
    let outcome = bob.sync_project("proj").await.map_err(anyhow::Error::msg)?;
    assert!(
        outcome.undecryptable.contains(&DecryptError::KeyNotFound),
        "expected KeyNotFound, got {:?}",
        outcome.undecryptable
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
    bob.adopt_project("proj", alice.doc_id("proj").unwrap())
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
    let alice = BeelayNode::bind_local(&alice_device, alice_auth)
        .await
        .map_err(anyhow::Error::msg)?;
    let bob = BeelayNode::bind_local(&bob_device, bob_auth)
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
        bob.fetch_blob(ticket).await.map_err(anyhow::Error::msg)?;
        print_rate("transfer", bytes.len(), start.elapsed());
        let start = std::time::Instant::now();
        let plain = bob
            .read_blob("proj", ticket)
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
    let (_, imposter_auth_id) = identities(dir.path(), "imposter");
    let host_auth = ProjectAuth::new(&host_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let imposter_auth = ProjectAuth::new(&imposter_auth_id)
        .await
        .map_err(anyhow::Error::msg)?;
    let host = BeelayNode::bind_local(&host_device, host_auth)
        .await
        .map_err(anyhow::Error::msg)?;

    // raw wire-level client so we control the hello bytes
    let endpoint = iroh::Endpoint::builder(presets::Minimal).bind().await?;
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

    // legitimate keyhive preamble (tag 1 both ways, cards then events)
    write_frame(
        &mut send,
        1,
        &imposter_auth
            .contact_card()
            .await
            .map_err(anyhow::Error::msg)?,
    )
    .await?;
    let (tag, card) = read_frame(&mut recv).await?;
    assert_eq!(tag, 1);
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

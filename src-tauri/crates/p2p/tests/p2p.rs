use anyhow::Result;
use automerge::{Automerge, ObjType, ROOT, ReadDoc, transaction::Transactable};
use linxiv_p2p::{DeviceIdentity, ShareNode, ShareTicket};

#[test]
fn identity_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("device.key");
    let first = DeviceIdentity::load_or_generate(&path).unwrap();
    let second = DeviceIdentity::load_or_generate(&path).unwrap();
    assert_eq!(first.endpoint_id(), second.endpoint_id());
}

#[test]
fn ticket_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let identity = DeviceIdentity::load_or_generate(dir.path().join("k")).unwrap();
    let ticket = ShareTicket::new(identity.endpoint_id(), "project-42");
    let s = ticket.to_string();
    let parsed: ShareTicket = s.parse().unwrap();
    assert_eq!(ticket, parsed);
    assert_eq!(parsed.project_id(), "project-42");
    assert_eq!(parsed.endpoint_id(), identity.endpoint_id());
}

fn get_str(doc: &Automerge, key: &str) -> Option<String> {
    doc.get(ROOT, key)
        .unwrap()
        .and_then(|(v, _)| v.into_string().ok())
}

#[tokio::test(flavor = "multi_thread")]
async fn two_nodes_sync() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let host_identity = DeviceIdentity::load_or_generate(dir.path().join("host.key"))?;
    let guest_identity = DeviceIdentity::load_or_generate(dir.path().join("guest.key"))?;
    // bind_local: no relays or discovery, tickets carry direct socket addrs.
    let host = ShareNode::bind_local(&host_identity).await?;
    let guest = ShareNode::bind_local(&guest_identity).await?;

    let mut doc = Automerge::new();
    doc.transact(|tx| {
        tx.put(ROOT, "title", "Shared Project")?;
        tx.put_object(ROOT, "papers", ObjType::List)?;
        Ok::<_, automerge::AutomergeError>(())
    })
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    host.register("proj", doc);

    // ticket round-trips through its string form, like a real paste.
    let ticket: ShareTicket = host.ticket("proj")?.to_string().parse()?;
    guest.join(&ticket).await?;

    let guest_doc = guest.doc("proj").expect("guest has the project after join");
    assert_eq!(
        get_str(&guest_doc, "title").as_deref(),
        Some("Shared Project")
    );

    // edit both sides, re-sync, converge.
    host.with_doc("proj", |d| {
        d.transact(|tx| tx.put(ROOT, "host_edit", "from host"))
            .map(|_| ())
            .unwrap();
    })
    .unwrap();
    guest
        .with_doc("proj", |d| {
            d.transact(|tx| tx.put(ROOT, "guest_edit", "from guest"))
                .map(|_| ())
                .unwrap();
        })
        .unwrap();

    guest.join(&ticket).await?;

    for (name, node) in [("host", &host), ("guest", &guest)] {
        let doc = node.doc("proj").unwrap();
        assert_eq!(
            get_str(&doc, "host_edit").as_deref(),
            Some("from host"),
            "{name} is missing the host edit"
        );
        assert_eq!(
            get_str(&doc, "guest_edit").as_deref(),
            Some("from guest"),
            "{name} is missing the guest edit"
        );
        assert_eq!(get_str(&doc, "title").as_deref(), Some("Shared Project"));
    }

    guest.shutdown().await?;
    host.shutdown().await?;
    Ok(())
}

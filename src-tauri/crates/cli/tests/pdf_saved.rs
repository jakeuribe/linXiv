//! Subprocess integration tests for `pdf list` / `pdf delete` via the real clap
//! dispatch. Files live in the managed pdf dir, so `files::delete_pdf`'s path-escape guard accepts them.

use std::process::Command;

use linxiv_core::models::PaperMetadata;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::storage;

fn arxiv_meta(source_id: &str, version: i64) -> PaperMetadata {
    serde_json::from_value(serde_json::json!({
        "source_id": source_id,
        "version": version,
        "title": "T",
        "authors": ["Alice"],
        "published": "2024-01-01",
        "summary": "s",
        "category": "cs.LG",
        "url": "http://arxiv.org/pdf/1v1",
        "source": "arxiv",
    }))
    .unwrap()
}

#[test]
fn pdf_list_then_delete_via_cli_dispatch() {
    let dir =
        std::env::temp_dir().join(format!("linxiv-cli-pdf-saved-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let pdf_dir = dir.join("pdfs");
    std::fs::create_dir_all(&pdf_dir).unwrap();

    let mut conn = storage::open(&dir.join("papers.db")).unwrap();
    storage::init_db(&conn).unwrap();

    // Two versions of one paper, both with a file on disk.
    for version in [1, 2] {
        svc_paper::save_paper_metadata(&mut conn, &arxiv_meta("arxiv:saved", version), None)
            .unwrap();
        let file = pdf_dir.join(svc_paper::pdf_on_disk_name("arxiv:saved", version));
        std::fs::write(&file, vec![b'x'; 100 * version as usize]).unwrap();
        svc_paper::set_has_pdf(&conn, "arxiv:saved", version, true).unwrap();
    }
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_linxiv-cli");
    let run = |args: &[&str]| {
        Command::new(bin)
            .env("LINXIV_DATA_DIR", &dir)
            .args(args)
            .output()
            .expect("failed to run linxiv")
    };

    // list reports the latest version only, with its on-disk size.
    let output = run(&["pdf", "list"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("expected valid JSON");
    let pdfs = json["pdfs"].as_array().unwrap();
    assert_eq!(pdfs.len(), 1);
    assert_eq!(pdfs[0]["source_id"], "arxiv:saved");
    assert_eq!(pdfs[0]["version"], 2);
    assert_eq!(pdfs[0]["size_bytes"], 200);

    // delete drops every version's file, not just the latest.
    let output = run(&["pdf", "delete", "arxiv:saved"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("expected valid JSON");
    assert_eq!(json["deleted"], true);
    for version in [1, 2] {
        assert!(!pdf_dir
            .join(svc_paper::pdf_on_disk_name("arxiv:saved", version))
            .exists());
    }

    let output = run(&["pdf", "list"]);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("expected valid JSON");
    assert_eq!(json["pdfs"].as_array().unwrap().len(), 0);

    // The paper row itself survives the PDF deletion.
    let output = run(&["paper", "get", "arxiv:saved"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    let output = run(&["pdf", "delete", "arxiv:missing"]);
    assert!(!output.status.success());

    let _ = std::fs::remove_dir_all(&dir);
}

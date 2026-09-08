//! Subprocess integration tests for `paper fetch-source` via the real clap dispatch.
//! Both cases resolve before any network await, so neither needs a live connection.

use std::process::Command;

use linxiv_core::models::PaperMetadata;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::storage;

fn arxiv_meta(source_id: &str, url: &str) -> PaperMetadata {
    serde_json::from_value(serde_json::json!({
        "source_id": source_id,
        "version": 1,
        "title": "T",
        "authors": ["Alice"],
        "published": "2024-01-01",
        "summary": "s",
        "category": "cs.LG",
        "url": url,
        "source": "arxiv",
    }))
    .unwrap()
}

#[test]
fn fetch_source_via_cli_dispatch() {
    let dir = std::env::temp_dir().join(format!(
        "linxiv-cli-fetch-source-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut conn = storage::open(&dir.join("papers.db")).unwrap();
    storage::init_db(&conn).unwrap();

    // Already indexed: fetch-source without --force should report indexed: false.
    svc_paper::save_paper_metadata(
        &mut conn,
        &arxiv_meta("arxiv:already", "http://arxiv.org/pdf/1v1"),
        None,
    )
    .unwrap();
    svc_paper::set_full_text(&mut conn, "arxiv:already", 1, "already have this").unwrap();

    // Abs-only URL: no /pdf/ source URL to derive a tarball from.
    svc_paper::save_paper_metadata(
        &mut conn,
        &arxiv_meta("arxiv:absonly", "http://arxiv.org/abs/2v1"),
        None,
    )
    .unwrap();
    drop(conn);

    let bin = env!("CARGO_BIN_EXE_linxiv-cli");

    let output = Command::new(bin)
        .env("LINXIV_DATA_DIR", &dir)
        .args(["paper", "fetch-source", "arxiv:already"])
        .output()
        .expect("failed to run paper fetch-source");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("expected valid JSON");
    assert_eq!(json["indexed"], false);

    let output = Command::new(bin)
        .env("LINXIV_DATA_DIR", &dir)
        .args(["paper", "fetch-source", "arxiv:absonly"])
        .output()
        .expect("failed to run paper fetch-source");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value = serde_json::from_str(&stderr).expect("expected valid JSON");
    assert!(json["error"].is_string());

    let _ = std::fs::remove_dir_all(&dir);
}

use std::process::Command;

#[test]
fn pdf_meta_missing_path_nonzero_exit() {
    let bin = env!("CARGO_BIN_EXE_linxiv-cli");
    let output = Command::new(bin)
        .arg(linxiv_core::service::paper_import::PDF_META_SUBCOMMAND)
        .arg("/nonexistent/path/that/does/not/exist.pdf")
        .output()
        .expect("failed to run pdf-meta");

    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn pdf_meta_garbage_file_ok() {
    let bin = env!("CARGO_BIN_EXE_linxiv-cli");

    // Create a small garbage file in /tmp with PID to avoid conflicts.
    let temp_file =
        std::env::temp_dir().join(format!("linxiv_test_garbage_{}.bin", std::process::id()));
    std::fs::write(&temp_file, b"not a pdf").expect("failed to write temp file");

    let output = Command::new(bin)
        .arg(linxiv_core::service::paper_import::PDF_META_SUBCOMMAND)
        .arg(&temp_file)
        .output()
        .expect("failed to run pdf-meta");

    // Cleanup before assertions to remove temp file even if test fails.
    let _ = std::fs::remove_file(&temp_file);

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("expected valid JSON");

    assert!(json["title"].is_null());
    assert!(json["authors"].is_null());
}

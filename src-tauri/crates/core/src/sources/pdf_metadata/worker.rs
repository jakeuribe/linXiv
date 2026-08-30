//! Subprocess boundary — a native libpdfium crash kills the worker, not the app.
//! Temp-file handoff, spawn/timeout process management, and the JSON IPC record.

use std::path::Path;

use super::extract::{extract_pdf_metadata, pdfium_lib_path, Extracted};

// ---------------------------------------------------------------------------
// Subprocess boundary — a native libpdfium crash kills the worker, not the app
// ---------------------------------------------------------------------------

/// The CLI subcommand the worker is invoked with (`linxiv pdf-meta <path>`).
/// `crates/cli` names its clap command from this same constant, so renaming the
/// subcommand is a one-const change both crates see at compile time.
pub const PDF_META_SUBCOMMAND: &str = "pdf-meta";

const WORKER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn sweep_stale_pdfmeta_temps() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| {
        if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
            let now = std::time::SystemTime::now();
            let one_hour = std::time::Duration::from_secs(3600);
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(name) = entry.file_name().into_string() {
                        if name.starts_with("linxiv-pdfmeta-") && name.ends_with(".pdf") {
                            if let Ok(modified) = metadata.modified() {
                                if let Ok(elapsed) = now.duration_since(modified) {
                                    if elapsed > one_hour {
                                        let _ = std::fs::remove_file(entry.path());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });
}

/// Extraction behind the crash boundary: routed through the worker subprocess
/// when one is found, else run in-process (the pre-boundary behavior).
pub(crate) fn extract_pdf_metadata_isolated(bytes: &[u8]) -> Extracted {
    extract_isolated_with(pdf_worker_path().as_deref(), bytes)
}

fn extract_isolated_with(worker: Option<&Path>, bytes: &[u8]) -> Extracted {
    match worker {
        // Worker failure (crash/timeout/garbage) degrades to all-None.
        Some(w) => extract_via_worker(w, bytes, WORKER_TIMEOUT).unwrap_or_default(),
        None => extract_pdf_metadata(bytes),
    }
}

/// Worker-process entry (`linxiv pdf-meta <path>`): direct in-process extraction,
/// emitted as one JSON object the parent parses back into `Extracted`.
pub fn extract_pdf_metadata_json(bytes: &[u8]) -> String {
    serde_json::to_string(&extract_pdf_metadata(bytes)).unwrap_or_else(|_| "{}".into())
}

/// Locate the worker (the CLI, which links this crate): `LINXIV_PDF_WORKER` env,
/// else a CLI binary next to the current exe — `linxiv-cli` in the cargo target
/// dir (dev), `linxiv` bundled-sidecar-adjacent (release). Every candidate takes
/// the same `is_file` check, so a broken path degrades to the in-process path.
fn pdf_worker_path() -> Option<std::path::PathBuf> {
    let env_var = std::env::var_os("LINXIV_PDF_WORKER");
    let env_set = env_var.is_some();
    let candidates: Vec<std::path::PathBuf> = match env_var {
        Some(p) => vec![p.into()],
        None => {
            let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
            let exe = if cfg!(windows) { ".exe" } else { "" };
            vec![
                dir.join(format!("linxiv-cli{exe}")),
                dir.join(format!("linxiv{exe}")),
            ]
        }
    };
    let result = candidates.into_iter().find(|p| p.is_file());
    if result.is_none() && env_set {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::warn!(
                "LINXIV_PDF_WORKER set but worker binary not found — \
                 falling back to in-process extraction"
            )
        });
    }
    result
}

/// Spawn `worker pdf-meta <tmp>` on a temp copy of `bytes`. None on any failure
/// (spawn, nonzero exit, timeout, unparseable stdout) — the caller defaults.
fn extract_via_worker(
    worker: &Path,
    bytes: &[u8],
    timeout: std::time::Duration,
) -> Option<Extracted> {
    sweep_stale_pdfmeta_temps();
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "linxiv-pdfmeta-{}-{}.pdf",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let write_result = opts.open(&tmp).and_then(|mut f| f.write_all(bytes));
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let result = run_worker(worker, &tmp, timeout);
    let _ = std::fs::remove_file(&tmp);
    result
}

fn run_worker(worker: &Path, pdf: &Path, timeout: std::time::Duration) -> Option<Extracted> {
    use std::process::Stdio;
    let mut cmd = std::process::Command::new(worker);
    cmd.arg(PDF_META_SUBCOMMAND)
        .arg(pdf)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Thread the resolved lib to the child so it binds the same libpdfium even
    // when the parent found it by exe-adjacent lookup rather than env.
    if let Some(lib) = pdfium_lib_path() {
        cmd.env("LINXIV_PDFIUM_LIB", lib);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "pdf-worker spawn failed");
            return None;
        }
    };

    let stdout = child.stdout.take().expect("stdout piped above");
    let stderr = child.stderr.take().expect("stderr piped above");
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        use std::io::Read;
        let _ = stdout.take(1_000_000).read_to_string(&mut out);
        out
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut err = String::new();
        use std::io::Read;
        let _ = stderr.take(1_000_000).read_to_string(&mut err);
        err
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                // Don't join the readers: an orphaned grandchild can hold the
                // pipes open past the deadline; the detached threads exit then.
                tracing::warn!("pdf-worker killed on timeout");
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        let _ = reader.join();
        let stderr_out = stderr_reader.join().unwrap_or_default();
        let stderr_snippet = stderr_out.chars().take(200).collect::<String>();
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if status.code().is_none() {
                if let Some(signal) = status.signal() {
                    tracing::warn!(
                        signal = signal,
                        stderr = %stderr_snippet,
                        "pdf-worker killed by signal"
                    );
                }
            } else {
                tracing::warn!(code = ?status.code(), stderr = %stderr_snippet, "pdf-worker exited non-zero");
            }
        }
        #[cfg(not(unix))]
        {
            tracing::warn!(code = ?status.code(), stderr = %stderr_snippet, "pdf-worker exited non-zero");
        }
        return None;
    }
    let out = reader.join().ok()?;
    match serde_json::from_str(&out) {
        Ok(extracted) => Some(extracted),
        Err(e) => {
            tracing::warn!(error = %e, len = out.len(), "pdf-worker JSON parse failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- subprocess boundary degrade paths (fake workers; no real segfault) ----

    #[cfg(unix)]
    fn fake_worker(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn worker_boundary_degrade_paths() {
        let dir = tempfile::tempdir().unwrap();
        let timeout = std::time::Duration::from_secs(10);

        // Healthy worker: the parsed record comes from the child's stdout.
        let ok = fake_worker(
            dir.path(),
            "ok.sh",
            r#"echo '{"title":"FROM_WORKER","authors":null,"doi":null,"arxiv_id":null,"year":2024}'"#,
        );
        let got = extract_via_worker(&ok, b"junk", timeout).expect("worker json parses");
        assert_eq!(got.title.as_deref(), Some("FROM_WORKER"));
        assert_eq!(got.year, Some(2024));

        // Nonzero exit (what a segfaulted child looks like) -> None -> default.
        let boom = fake_worker(dir.path(), "boom.sh", "exit 139");
        assert_eq!(extract_via_worker(&boom, b"junk", timeout), None);
        assert_eq!(
            extract_isolated_with(Some(&boom), b"junk"),
            Extracted::default()
        );

        // Garbage stdout -> None.
        let garbage = fake_worker(dir.path(), "garbage.sh", "echo not-json");
        assert_eq!(extract_via_worker(&garbage, b"junk", timeout), None);

        // Hung child is killed at the deadline -> None (bounded, not forever).
        let hang = fake_worker(dir.path(), "hang.sh", "sleep 30");
        let t0 = std::time::Instant::now();
        assert_eq!(
            extract_via_worker(&hang, b"junk", std::time::Duration::from_millis(300)),
            None
        );
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));

        // No worker found -> in-process fallback (junk -> default, no crash).
        assert_eq!(
            extract_isolated_with(None, b"%PDF junk"),
            Extracted::default()
        );
        // Worker routing via the dispatch seam.
        assert_eq!(
            extract_isolated_with(Some(&ok), b"junk").title.as_deref(),
            Some("FROM_WORKER")
        );
    }
}

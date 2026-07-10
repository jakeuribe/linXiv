//! On-disk LaTeX vault backing the embedded TeXbrain editor's filesystem.
//! Rust port of `service/vault.py`.
//!
//! Each embedded-editor project owns one directory tree (its `vault_root`, keyed
//! by note id under `vault_dir()/note_<NOTE_SK>/`). The TeXbrain editor, in an
//! iframe, drives its FileSystemDirectoryHandle over a postMessage RPC; the host
//! forwards each [`FsOp`] to [`run_fs_op`] here, which returns the matching
//! [`FsResult`]. The wire shapes mirror src/lib/editorBridgeTypes.ts.
//!
//! DI: `vault_root` is a PARAMETER, not read from config — the binary layer maps
//! `note_id` -> `vault_dir()/note_<id>` and passes the resolved path in (the
//! tests use a tempdir, never config). No DB here; this module is pure FS.
//!
//! Security (trust boundary — ported exactly, do not simplify): every op resolves
//! its path through [`safe_path`], which rejects absolute paths and any `..`
//! traversal BEFORE the join, then asserts the result stays under `vault_root`.
//! text-vs-binary is classified by EXTENSION (matching the TeXbrain guest's
//! readDirRecursive), NOT by utf-8 decodability — otherwise a latin-1 `.tex`
//! would ship as base64 and the guest's `.text()` would mojibake it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Map a filesystem error to `Internal` (HTTP 500) — Python lets `OSError`
/// bubble to the FastAPI 500 handler. (core's `CoreError` has no blanket
/// `From<io::Error>`, so each FS call routes through this.)
fn io(e: std::io::Error) -> CoreError {
    CoreError::Internal(e.to_string())
}

// ── wire types (vendored from editorBridgeTypes.ts FsOp/FsResult) ───────────────
// Tagged by `kind`; camelCase matches the wire ("readFile"/"writeFile"/"mkdir").
// An unknown kind fails at deserialize time (-> the binary layer maps the serde
// error to BadRequest), which is why `run_fs_op` needs no unknown-kind arm.

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FsOp {
    List {
        #[serde(default)]
        path: String,
    },
    ReadFile {
        #[serde(default)]
        path: String,
    },
    WriteFile {
        #[serde(default)]
        path: String,
        #[serde(default)]
        data: String,
        #[serde(default)]
        binary: bool,
    },
    Mkdir {
        #[serde(default)]
        path: String,
    },
    Remove {
        #[serde(default)]
        path: String,
        #[serde(default)]
        recursive: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirEntry {
    pub name: String,
    /// "directory" | "file" (the guest re-joins each basename to the parent).
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FsResult {
    List { entries: Vec<DirEntry> },
    ReadFile { data: String, binary: bool },
    Ok,
}

// ── path resolution + containment guard ─────────────────────────────────────────

/// Resolve a root-relative editor path inside the vault, or error.
///
/// Rejects absolute paths and any `..` traversal, then asserts the resolved path
/// stays within `vault_root`. `relpath == ""` resolves to the vault root itself.
/// Python's `ValueError` maps to [`CoreError::BadRequest`] (HTTP 400).
pub fn safe_path(vault_root: &Path, relpath: &str) -> Result<PathBuf> {
    let raw = relpath.replace('\\', "/");
    if raw.starts_with('/') {
        return Err(CoreError::BadRequest(
            "absolute paths are not allowed".into(),
        ));
    }
    let parts: Vec<&str> = raw
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.iter().any(|p| *p == "..") {
        return Err(CoreError::BadRequest(
            "path traversal is not allowed".into(),
        ));
    }
    let mut target = vault_root.to_path_buf();
    for p in &parts {
        target.push(p);
    }
    // Containment belt: parts are already `..`/absolute-free, so the lexical
    // prefix check always holds — but it stays as the explicit trust-boundary
    // assert (Python's is_relative_to).
    if !parts.is_empty() && !target.starts_with(vault_root) {
        return Err(CoreError::BadRequest("path escapes the vault root".into()));
    }
    Ok(target)
}

// ── extension-based text/binary classifier (matches the TeXbrain guest) ─────────

const TEXT_EXTS: &[&str] = &[
    "tex", "sty", "cls", "bib", "bst", "def", "cfg", "fd", "dtx", "ins", "ltx", "txt", "bbx",
    "cbx", "lbx",
];
const BINARY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "pdf", "eps", "svg", "gif", "bmp", "tfm", "pfb", "vf", "map", "enc",
    "otf", "ttf",
];

/// `Some(true)`/`Some(false)` per the editor's extension sets; `None` for an
/// unknown extension (the caller then falls back to utf-8 decodability).
fn ext_is_text(relpath: &str) -> Option<bool> {
    // Mirror Python's `Path(relpath).suffix.lower().lstrip(".")`: the substring
    // after the last '.' in the basename (Path::extension drops a leading-dot
    // "dotfile" the same way `.suffix` returns "" for it).
    let ext = Path::new(relpath)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let ext = ext.as_deref()?;
    if TEXT_EXTS.contains(&ext) {
        Some(true)
    } else if BINARY_EXTS.contains(&ext) {
        Some(false)
    } else {
        None
    }
}

// ── individual ops (error on failure; the route maps to HTTP status) ────────────

/// List immediate children (BASENAMES only). A missing dir lists as empty rather
/// than erroring, so a freshly-mounted vault doesn't break the editor's scan.
pub fn list_dir(vault_root: &Path, relpath: &str) -> Result<FsResult> {
    let target = safe_path(vault_root, relpath)?;
    let mut entries: Vec<DirEntry> = Vec::new();
    if target.is_dir() {
        for child in std::fs::read_dir(&target).map_err(io)? {
            let child = child.map_err(io)?;
            let kind = if child.file_type().map_err(io)?.is_dir() {
                "directory"
            } else {
                "file"
            };
            entries.push(DirEntry {
                name: child.file_name().to_string_lossy().into_owned(),
                kind: kind.into(),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Ok(FsResult::List { entries })
}

/// Read a file, classifying text-vs-binary by extension to match the editor.
/// Text-extension files return a raw string (utf-8, falling back to latin-1 so
/// non-utf-8 LaTeX source is not corrupted into base64). Binary-extension files
/// return base64. Unknown extensions fall back to utf-8 decodability.
pub fn read_file(vault_root: &Path, relpath: &str) -> Result<FsResult> {
    let target = safe_path(vault_root, relpath)?;
    if !target.is_file() {
        return Err(CoreError::NotFound(relpath.to_string()));
    }
    let raw = std::fs::read(&target).map_err(io)?;
    match ext_is_text(relpath) {
        Some(true) => {
            // utf-8, else latin-1 (each byte -> U+00xx); save round-trips to utf-8.
            let text = String::from_utf8(raw)
                .unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
            Ok(FsResult::ReadFile {
                data: text,
                binary: false,
            })
        }
        Some(false) => Ok(FsResult::ReadFile {
            data: b64_encode(&raw),
            binary: true,
        }),
        None => match String::from_utf8(raw) {
            Ok(text) => Ok(FsResult::ReadFile {
                data: text,
                binary: false,
            }),
            Err(e) => Ok(FsResult::ReadFile {
                data: b64_encode(&e.into_bytes()),
                binary: true,
            }),
        },
    }
}

/// Write a file, creating parent dirs. `data` is base64 when `binary`, else raw
/// text. An empty string materializes a zero-length file (the create-empty path).
pub fn write_file(vault_root: &Path, relpath: &str, data: &str, binary: bool) -> Result<FsResult> {
    let target = safe_path(vault_root, relpath)?;
    if target == *vault_root {
        return Err(CoreError::BadRequest(
            "cannot write the vault root as a file".into(),
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    if binary {
        std::fs::write(&target, b64_decode(data)?).map_err(io)?;
    } else {
        std::fs::write(&target, data.as_bytes()).map_err(io)?;
    }
    Ok(FsResult::Ok)
}

pub fn make_dir(vault_root: &Path, relpath: &str) -> Result<FsResult> {
    let target = safe_path(vault_root, relpath)?;
    std::fs::create_dir_all(&target).map_err(io)?;
    Ok(FsResult::Ok)
}

pub fn remove_entry(vault_root: &Path, relpath: &str, recursive: bool) -> Result<FsResult> {
    let target = safe_path(vault_root, relpath)?;
    if target == *vault_root {
        return Err(CoreError::BadRequest("cannot remove the vault root".into()));
    }
    if target.is_dir() {
        if recursive {
            std::fs::remove_dir_all(&target).map_err(io)?;
        } else {
            // Errors if non-empty — mirrors removeEntry(recursive: false).
            std::fs::remove_dir(&target).map_err(io)?;
        }
    } else if target.exists() {
        std::fs::remove_file(&target).map_err(io)?;
    } else {
        return Err(CoreError::NotFound(relpath.to_string()));
    }
    Ok(FsResult::Ok)
}

// ── op dispatch (one entry point for the /fs route) ─────────────────────────────

/// Dispatch one [`FsOp`] to the matching disk op and return the [`FsResult`].
/// `BadRequest` (-> 400) for bad input, `NotFound` (-> 404) for missing files.
pub fn run_fs_op(vault_root: &Path, op: &FsOp) -> Result<FsResult> {
    match op {
        FsOp::List { path } => list_dir(vault_root, path),
        FsOp::ReadFile { path } => read_file(vault_root, path),
        FsOp::WriteFile { path, data, binary } => write_file(vault_root, path, data, *binary),
        FsOp::Mkdir { path } => make_dir(vault_root, path),
        FsOp::Remove { path, recursive } => remove_entry(vault_root, path, *recursive),
    }
}

/// Every file in the vault as root-relative posix paths (no content read). Used
/// to resolve/repair the project's main file. A missing root lists as empty.
pub fn list_files(vault_root: &Path) -> Result<Vec<String>> {
    if !vault_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    walk_files(vault_root, vault_root, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for child in std::fs::read_dir(dir).map_err(io)? {
        let child = child.map_err(io)?;
        let path = child.path();
        if child.file_type().map_err(io)?.is_dir() {
            walk_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            // posix-relative: components joined with '/'.
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(rel.join("/"));
        }
    }
    Ok(())
}

/// Remove an editor project's entire vault tree (best-effort, like Python's
/// `shutil.rmtree(..., ignore_errors=True)`).
pub fn delete_vault(vault_root: &Path) {
    if vault_root.is_dir() {
        let _ = std::fs::remove_dir_all(vault_root);
    }
}

// ── base64 (standard alphabet, padded) ──────────────────────────────────────────

fn b64_encode(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn b64_decode(s: &str) -> Result<Vec<u8>> {
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for c in s.bytes() {
        let v: u8 = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return Err(CoreError::BadRequest("invalid base64 data".into())),
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

// ── tests (tempdir asserts real FS effects + the security rejections) ───────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn safe_path_resolves_relative_and_empty() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert_eq!(safe_path(root, "").unwrap(), root);
        assert_eq!(
            safe_path(root, "a/b.tex").unwrap(),
            root.join("a").join("b.tex")
        );
        // "." and "" components are dropped, like Python.
        assert_eq!(
            safe_path(root, "./a//b.tex").unwrap(),
            root.join("a").join("b.tex")
        );
    }

    #[test]
    fn safe_path_rejects_absolute_and_traversal() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        for bad in ["/etc/passwd", "../escape", "a/../../etc", "a/b/../../../x"] {
            assert!(safe_path(root, bad).is_err(), "should reject {bad}");
        }
        // backslashes normalize to '/', so a backslash traversal is caught too.
        assert!(safe_path(root, "..\\escape").is_err());
    }

    #[test]
    fn classifier_is_extension_based() {
        assert_eq!(ext_is_text("main.tex"), Some(true));
        assert_eq!(ext_is_text("REFS.BIB"), Some(true)); // case-insensitive
        assert_eq!(ext_is_text("fig.png"), Some(false));
        assert_eq!(ext_is_text("font.otf"), Some(false));
        assert_eq!(ext_is_text("Makefile"), None); // unknown
        assert_eq!(ext_is_text("noext"), None);
    }

    #[test]
    fn latin1_tex_is_text_not_base64() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // 0xE9 = 'é' in latin-1, invalid as a lone utf-8 byte.
        std::fs::write(root.join("a.tex"), [b'c', b'a', b'f', 0xE9]).unwrap();
        match read_file(root, "a.tex").unwrap() {
            FsResult::ReadFile { data, binary } => {
                assert!(!binary, "a .tex must ride as text, not base64");
                assert_eq!(data, "café"); // latin-1 0xE9 -> U+00E9
            }
            other => panic!("expected readFile, got {other:?}"),
        }
    }

    #[test]
    fn binary_ext_round_trips_through_base64() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let bytes: Vec<u8> = (0u8..=255).collect();
        write_file(root, "img.png", &b64_encode(&bytes), true).unwrap();
        // bytes hit disk raw, not base64-text.
        assert_eq!(std::fs::read(root.join("img.png")).unwrap(), bytes);
        match read_file(root, "img.png").unwrap() {
            FsResult::ReadFile { data, binary } => {
                assert!(binary);
                assert_eq!(b64_decode(&data).unwrap(), bytes);
            }
            other => panic!("expected readFile, got {other:?}"),
        }
    }

    #[test]
    fn write_text_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_file(
            root,
            "deep/nested/main.tex",
            "\\documentclass{article}",
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("deep/nested/main.tex")).unwrap(),
            "\\documentclass{article}"
        );
    }

    #[test]
    fn write_empty_makes_zero_length_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_file(root, "empty.tex", "", false).unwrap();
        assert_eq!(std::fs::metadata(root.join("empty.tex")).unwrap().len(), 0);
    }

    #[test]
    fn write_and_remove_reject_vault_root() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert!(write_file(root, "", "x", false).is_err());
        assert!(remove_entry(root, "", false).is_err());
    }

    #[test]
    fn list_dir_sorts_and_tags_kind_missing_is_empty() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub/inner")).unwrap();
        std::fs::write(root.join("b.tex"), "x").unwrap();
        std::fs::write(root.join("a.tex"), "x").unwrap();
        match list_dir(root, "").unwrap() {
            FsResult::List { entries } => {
                assert_eq!(
                    entries,
                    vec![
                        DirEntry {
                            name: "a.tex".into(),
                            kind: "file".into()
                        },
                        DirEntry {
                            name: "b.tex".into(),
                            kind: "file".into()
                        },
                        DirEntry {
                            name: "sub".into(),
                            kind: "directory".into()
                        },
                    ]
                );
            }
            other => panic!("expected list, got {other:?}"),
        }
        // missing subdir -> empty, not an error.
        assert_eq!(
            list_dir(root, "does/not/exist").unwrap(),
            FsResult::List { entries: vec![] }
        );
    }

    #[test]
    fn remove_nonrecursive_fails_on_nonempty_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d/f.tex"), "x").unwrap();
        assert!(remove_entry(root, "d", false).is_err());
        remove_entry(root, "d", true).unwrap(); // recursive succeeds
        assert!(!root.join("d").exists());
    }

    #[test]
    fn remove_missing_errors_file_removes() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert!(remove_entry(root, "gone.tex", false).is_err());
        std::fs::write(root.join("here.tex"), "x").unwrap();
        remove_entry(root, "here.tex", false).unwrap();
        assert!(!root.join("here.tex").exists());
    }

    #[test]
    fn list_files_is_recursive_posix_sorted() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/z.tex"), "x").unwrap();
        std::fs::write(root.join("top.tex"), "x").unwrap();
        assert_eq!(list_files(root).unwrap(), vec!["a/b/z.tex", "top.tex"]);
        // missing root -> empty.
        assert_eq!(
            list_files(&root.join("nope")).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn run_fs_op_dispatches() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        run_fs_op(root, &FsOp::Mkdir { path: "d".into() }).unwrap();
        assert!(root.join("d").is_dir());
        run_fs_op(
            root,
            &FsOp::WriteFile {
                path: "d/x.tex".into(),
                data: "hi".into(),
                binary: false,
            },
        )
        .unwrap();
        let r = run_fs_op(
            root,
            &FsOp::ReadFile {
                path: "d/x.tex".into(),
            },
        )
        .unwrap();
        assert_eq!(
            r,
            FsResult::ReadFile {
                data: "hi".into(),
                binary: false
            }
        );
    }

    #[test]
    fn fs_op_deserializes_from_wire_and_unknown_kind_fails() {
        let op: FsOp =
            serde_json::from_str(r#"{"kind":"writeFile","path":"a.tex","data":"x"}"#).unwrap();
        match op {
            FsOp::WriteFile { path, data, binary } => {
                assert_eq!(
                    (path.as_str(), data.as_str(), binary),
                    ("a.tex", "x", false)
                );
            }
            other => panic!("got {other:?}"),
        }
        // unknown kind -> serde error (the seam maps it to BadRequest).
        assert!(serde_json::from_str::<FsOp>(r#"{"kind":"chmod","path":"a"}"#).is_err());
    }

    #[test]
    fn delete_vault_removes_tree() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("note_1");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.tex"), "x").unwrap();
        delete_vault(&root);
        assert!(!root.exists());
        delete_vault(&root); // idempotent / best-effort on missing
    }
}

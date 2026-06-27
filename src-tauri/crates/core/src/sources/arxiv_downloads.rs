//! arxiv_downloads — stream the PDF / TeX-source tarball and extract `.tex` text.
//! Port of `sources/arxiv_downloads.py`. Plan §5.4.
//!
//! Pure pieces (fixture-tested below): `default_filename`, `strip_tex_noise`,
//! `extract_source` (gzip+tar, keep `*.tex`, comment-strip, path-guard against
//! `../`/absolute escape), and the `/pdf/`->`/src/` source rewrite.
//!
//! The async fetch wrappers stream `reqwest` -> `tokio::fs` and lean on
//! `http::arxiv_get` for pacing + the `export.arxiv.org` host rewrite + the
//! arXiv host allowlist/redirect guard (so we never rebuild any of that here).
//! Dest dirs are DI params — nothing reads config.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;
use tokio::io::AsyncWriteExt;

use crate::error::{CoreError, Result};
use crate::sources::http;

// ---------------------------------------------------------------------------
// URL / filename helpers
// ---------------------------------------------------------------------------

/// Rewrite an arXiv `/pdf/` URL to its `/src/` (TeX tarball) sibling.
/// Mirrors Python's `pdf_url.replace('/pdf/', '/src/')`.
fn pdf_to_src(url: &str) -> String {
    url.replace("/pdf/", "/src/")
}

/// Safe default filename from a paper id or URL: take the last path segment,
/// replace every char outside `[A-Za-z0-9_.-]` with `_`, append `.<extension>`.
/// Mirrors `_default_filename` (`re.sub(r'[^\w.\-]', '_', tail)`).
pub fn default_filename(id_or_url: &str, extension: &str) -> String {
    let tail = id_or_url.rsplit('/').next().unwrap_or(id_or_url);
    let safe: String = tail
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.{extension}")
}

// ---------------------------------------------------------------------------
// TeX noise stripping  (regex-free: the `regex` crate is not a dependency, and
// its comment pattern needs a lookbehind `regex` doesn't support anyway)
// ---------------------------------------------------------------------------

/// Drop a TeX line comment: everything from the first un-escaped `%` to EOL.
/// A `%` is a comment iff the char immediately before it is not `\` — exactly
/// the `(?<!\\)%` rule, so `\%` (escaped percent) is preserved.
fn strip_line_comment(line: &str) -> &str {
    let b = line.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'%' && (i == 0 || b[i - 1] != b'\\') {
            return &line[..i];
        }
    }
    line
}

/// The boilerplate commands removed wholesale (`\cmd{...}`), longest-first so
/// `\bibliographystyle{}` is matched before the `bibliography` prefix — the
/// same precedence as the Python alternation's leftmost rule.
const TEX_COMMANDS: &[&str] = &[
    "bibliographystyle",
    "documentclass",
    "bibliography",
    "usepackage",
    "include",
    "input",
    "label",
    "cite",
    "ref",
];

/// Remove `\<cmd>{<no-brace>}` for each command in `TEX_COMMANDS`.
/// `{...}` is non-greedy to the first `}` (mirrors `\{[^}]*\}`); a `\cmd` with
/// no following `{...}` is left untouched.
fn strip_commands(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < b.len() {
        if b[i] == b'\\' {
            for cmd in TEX_COMMANDS {
                let start = i + 1;
                let end = start + cmd.len();
                if end < b.len() && b[end] == b'{' && &s[start..end] == *cmd {
                    if let Some(rel) = s[end + 1..].find('}') {
                        i = end + 1 + rel + 1; // skip the whole `\cmd{...}`
                        continue 'outer;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Remove TeX comments then boilerplate commands. Port of `_strip_tex_noise`.
pub fn strip_tex_noise(text: &str) -> String {
    let no_comments: Vec<&str> = text.split('\n').map(strip_line_comment).collect();
    strip_commands(&no_comments.join("\n"))
}

// ---------------------------------------------------------------------------
// TeX source extraction
// ---------------------------------------------------------------------------

/// PATH-GUARD: reject any tar member that is absolute or contains a `..`
/// component, so extraction can never reference a path outside the archive.
fn member_is_safe(name: &str) -> bool {
    let p = Path::new(name);
    !p.is_absolute()
        && !name.starts_with('/')
        && !p.components().any(|c| matches!(c, Component::ParentDir))
}

/// Extract TeX source from a `.tar.gz` at `tarpath`, returning concatenated,
/// noise-stripped plain text. `.tex` members only, root files before nested
/// (stable sort by `/` depth), unsafe paths skipped. Any tar/io error -> `""`.
/// Port of `extract_source` (we read members straight from the stream rather
/// than extracting to a temp dir — the path-guard makes a temp dir unneeded).
pub fn extract_source(tarpath: &Path) -> String {
    extract_source_inner(tarpath).unwrap_or_default()
}

fn extract_source_inner(tarpath: &Path) -> Result<String> {
    let file = std::fs::File::open(tarpath)
        .map_err(|e| CoreError::Internal(format!("open {tarpath:?}: {e}")))?;
    let mut archive = Archive::new(GzDecoder::new(file));

    let mut tex: Vec<(String, String)> = Vec::new();
    let entries = archive
        .entries()
        .map_err(|e| CoreError::Internal(format!("read tar: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| CoreError::Internal(format!("tar entry: {e}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry
            .path()
            .map_err(|e| CoreError::Internal(format!("tar path: {e}")))?
            .to_string_lossy()
            .into_owned();
        if !name.ends_with(".tex") || !member_is_safe(&name) {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| CoreError::Internal(format!("read member {name:?}: {e}")))?;
        tex.push((name, String::from_utf8_lossy(&bytes).into_owned()));
    }

    if tex.is_empty() {
        return Ok(String::new());
    }
    tex.sort_by_key(|(name, _)| name.matches('/').count()); // stable: root first
    let combined = tex
        .iter()
        .map(|(_, c)| c.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(strip_tex_noise(&combined))
}

// ---------------------------------------------------------------------------
// PDF / source download  (async, streamed; integration-tested via http later)
// ---------------------------------------------------------------------------

/// Stream `arxiv_get(url)` to `dest_dir/<filename>` atomically (tmp -> rename).
async fn stream_to(url: &str, dest_dir: &Path, filename: &str, data_dir: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dest_dir)
        .await
        .map_err(|e| CoreError::Internal(format!("mkdir {dest_dir:?}: {e}")))?;

    let mut resp = http::arxiv_get(url, data_dir).await?;
    if !resp.status().is_success() {
        return Err(CoreError::Upstream(format!(
            "GET {url:?} -> HTTP {}",
            resp.status().as_u16()
        )));
    }

    let dest = dest_dir.join(filename);
    let tmp = dest_dir.join(format!(".{filename}.part"));
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| CoreError::Internal(format!("create {tmp:?}: {e}")))?;
    // chunk() streams the body without pulling in a `futures` Stream dependency.
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CoreError::Upstream(format!("stream {url:?}: {e}")))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| CoreError::Internal(format!("write {tmp:?}: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| CoreError::Internal(format!("flush {tmp:?}: {e}")))?;
    drop(file);
    tokio::fs::rename(&tmp, &dest)
        .await
        .map_err(|e| CoreError::Internal(format!("rename -> {dest:?}: {e}")))?;
    Ok(dest)
}

/// Download a paper's PDF into `dest_dir`. `pdf_url` is the arXiv PDF URL
/// (`arxiv_get` rewrites the host to `export.arxiv.org`). Returns the path.
pub async fn download_pdf(pdf_url: &str, dest_dir: &Path, data_dir: &Path) -> Result<PathBuf> {
    let filename = default_filename(pdf_url, "pdf");
    stream_to(pdf_url, dest_dir, &filename, data_dir).await
}

/// Download a paper's TeX-source tarball into `dest_dir`. Derived from the PDF
/// URL by `/pdf/`->`/src/`. Returns the path written.
pub async fn download_source(pdf_url: &str, dest_dir: &Path, data_dir: &Path) -> Result<PathBuf> {
    let src_url = pdf_to_src(pdf_url);
    let filename = default_filename(pdf_url, "tar.gz");
    stream_to(&src_url, dest_dir, &filename, data_dir).await
}

// ---------------------------------------------------------------------------
// PDF housekeeping  (sync FS; ports cleanup_pdfs / saved_pdfs_size)
// ---------------------------------------------------------------------------

/// Absolute path without requiring the file to exist (Python's `os.path.abspath`).
fn abs(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Delete every `*.pdf` in `dir` whose absolute path is not in `keep`.
/// A file that can't be removed (locked by a viewer) is skipped, not fatal.
/// Returns the paths deleted. Port of `cleanup_pdfs`.
pub fn cleanup_pdfs(dir: &Path, keep: &HashSet<PathBuf>) -> Result<Vec<PathBuf>> {
    let keep_abs: HashSet<PathBuf> = keep.iter().map(|p| abs(p)).collect();
    let mut deleted = Vec::new();
    let read = std::fs::read_dir(dir)
        .map_err(|e| CoreError::Internal(format!("read_dir {dir:?}: {e}")))?;
    for entry in read {
        let entry = entry.map_err(|e| CoreError::Internal(format!("dir entry: {e}")))?;
        let path = entry.path();
        let is_pdf = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false);
        if !is_pdf || keep_abs.contains(&abs(&path)) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            deleted.push(path);
        }
    }
    Ok(deleted)
}

/// Total byte size of the existing files among `paths`. Port of `saved_pdfs_size`.
pub fn saved_pdfs_size(paths: &HashSet<PathBuf>) -> u64 {
    paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

// ---------------------------------------------------------------------------
// Tests — pure parsers against synthetic tar fixtures (no network).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;

    fn make_tarball(files: &[(&str, &str)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let enc = GzEncoder::new(&mut bytes, Compression::default());
            let mut builder = tar::Builder::new(enc);
            for (name, content) in files {
                let data = content.as_bytes();
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mode(0o644);
                builder.append_data(&mut header, name, data).unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap();
        }
        bytes
    }

    fn write_tarball(dir: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, make_tarball(files)).unwrap();
        p
    }

    // -- default_filename / pdf_to_src --------------------------------------

    #[test]
    fn default_filename_stem_and_extension() {
        assert_eq!(
            default_filename("http://arxiv.org/abs/2204.12985v4", "pdf"),
            "2204.12985v4.pdf"
        );
        assert!(default_filename("x/2204.12985v4", "tar.gz").ends_with(".tar.gz"));
    }

    #[test]
    fn default_filename_sanitizes_unsafe_chars() {
        let out = default_filename("http://arxiv.org/abs/bad id:here", "pdf");
        assert!(!out.contains(' '));
        assert!(!out.contains(':'));
        assert_eq!(out, "bad_id_here.pdf");
    }

    #[test]
    fn pdf_to_src_rewrite() {
        let out = pdf_to_src("https://arxiv.org/pdf/2204.12985v4");
        assert!(out.contains("/src/"));
        assert!(!out.contains("/pdf/"));
    }

    // -- strip_tex_noise ----------------------------------------------------

    #[test]
    fn strips_comment_keeps_next_line() {
        let out = strip_tex_noise("some text % this is a comment\nnext line");
        assert!(!out.contains("this is a comment"));
        assert!(out.contains("next line"));
    }

    #[test]
    fn preserves_escaped_percent() {
        assert!(strip_tex_noise(r"50\% of the time").contains(r"50\%"));
    }

    #[test]
    fn removes_boilerplate_commands() {
        assert!(!strip_tex_noise(r"\usepackage{amsmath} some text").contains(r"\usepackage"));
        assert!(strip_tex_noise(r"\usepackage{amsmath} some text").contains("some text"));
        assert!(!strip_tex_noise(r"\documentclass{article} rest").contains(r"\documentclass"));
        assert!(!strip_tex_noise(r"See \cite{smith2020} for details.").contains(r"\cite"));
        assert!(!strip_tex_noise(r"\label{fig:main} caption").contains(r"\label"));
        assert!(!strip_tex_noise(r"Figure \ref{fig:main} shows").contains(r"\ref"));
        assert!(!strip_tex_noise(r"\input{sections/intro} rest").contains(r"\input"));
        assert!(!strip_tex_noise(r"\include{appendix} content").contains(r"\include"));
        let bib = strip_tex_noise(r"\bibliographystyle{plain}\bibliography{refs}");
        assert!(!bib.contains(r"\bibliographystyle"));
        assert!(!bib.contains(r"\bibliography"));
    }

    #[test]
    fn keeps_surrounding_prose() {
        let out = strip_tex_noise(r"See \cite{smith2020} for details.");
        assert!(out.contains("See"));
        assert!(out.contains("for details."));
        let prose = "This is a normal paragraph about deep learning.";
        assert_eq!(strip_tex_noise(prose), prose);
    }

    // -- member_is_safe (PATH-GUARD) ---------------------------------------

    #[test]
    fn path_guard_rejects_escapes_accepts_normal() {
        assert!(!member_is_safe("../malicious.tex"));
        assert!(!member_is_safe("/etc/evil.tex"));
        assert!(!member_is_safe("a/../../etc/x.tex"));
        assert!(member_is_safe("main.tex"));
        assert!(member_is_safe("subdir/section.tex"));
    }

    // -- extract_source -----------------------------------------------------

    #[test]
    fn extract_returns_tex_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tarball(
            dir.path(),
            "src.tar.gz",
            &[("main.tex", "Hello world in TeX.")],
        );
        assert!(extract_source(&p).contains("Hello world in TeX."));
    }

    #[test]
    fn extract_strips_comments() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tarball(
            dir.path(),
            "src.tar.gz",
            &[("main.tex", "content % inline comment\nnext line")],
        );
        let out = extract_source(&p);
        assert!(!out.contains("inline comment"));
        assert!(out.contains("next line"));
    }

    #[test]
    fn extract_root_before_nested_and_concatenates() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tarball(
            dir.path(),
            "src.tar.gz",
            &[("subdir/section.tex", "NESTED"), ("main.tex", "ROOT")],
        );
        let out = extract_source(&p);
        assert!(out.contains("ROOT") && out.contains("NESTED"));
        assert!(out.find("ROOT").unwrap() < out.find("NESTED").unwrap());
    }

    #[test]
    fn extract_empty_when_no_tex() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tarball(dir.path(), "src.tar.gz", &[("readme.txt", "no tex here")]);
        assert_eq!(extract_source(&p), "");
    }

    #[test]
    fn extract_empty_for_corrupt_tar() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.tar.gz");
        std::fs::write(&p, b"this is not a tarball").unwrap();
        assert_eq!(extract_source(&p), "");
    }

    // -- cleanup_pdfs / saved_pdfs_size ------------------------------------

    #[test]
    fn cleanup_deletes_unkept_pdfs_only() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.pdf");
        let keep = dir.path().join("keep.pdf");
        let txt = dir.path().join("readme.txt");
        std::fs::write(&old, b"data").unwrap();
        std::fs::write(&keep, b"data").unwrap();
        std::fs::write(&txt, b"hello").unwrap();

        let keep_set: HashSet<PathBuf> = [keep.clone()].into_iter().collect();
        let deleted = cleanup_pdfs(dir.path(), &keep_set).unwrap();
        assert_eq!(deleted, vec![old.clone()]);
        assert!(!old.exists());
        assert!(keep.exists());
        assert!(txt.exists()); // non-pdf untouched
    }

    #[test]
    fn cleanup_empty_keep_deletes_all_pdfs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.pdf"), b"x").unwrap();
        std::fs::write(dir.path().join("b.pdf"), b"x").unwrap();
        let deleted = cleanup_pdfs(dir.path(), &HashSet::new()).unwrap();
        assert_eq!(deleted.len(), 2);
    }

    #[test]
    fn saved_size_sums_existing_skips_missing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pdf");
        let b = dir.path().join("b.pdf");
        std::fs::write(&a, b"12345").unwrap(); // 5
        std::fs::write(&b, b"1234567890").unwrap(); // 10
        let missing = dir.path().join("ghost.pdf");
        let set: HashSet<PathBuf> = [a, b, missing].into_iter().collect();
        assert_eq!(saved_pdfs_size(&set), 15);
        assert_eq!(saved_pdfs_size(&HashSet::new()), 0);
    }
}

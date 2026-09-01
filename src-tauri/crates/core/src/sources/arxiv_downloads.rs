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
//!
//! Three size ceilings bound what an upstream response can cost us:
//! `MAX_DOWNLOAD_BYTES` on the streamed body, `MAX_DECOMPRESSED_BYTES` on bytes
//! pulled through the gzip decoder, and `MAX_TEX_BYTES` on the TeX read out of
//! the tarball. `MAX_TEX_BYTES` and `MAX_DECOMPRESSED_BYTES` are both
//! unit-tested; only `MAX_DOWNLOAD_BYTES` is not — `arxiv_get` rewrites the
//! host to `export.arxiv.org` and enforces the arXiv allowlist, so a loopback
//! wiremock cannot drive the download path without weakening that guard (the same
//! constraint `service::files`' download tests document).

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

/// Map an arXiv `…/pdf/<id>v<n>` URL to the matching object on the free public
/// GCS mirror (`gs://arxiv-dataset`, served over HTTPS without auth) so a PDF
/// view skips arXiv's rate limit. Returns None unless the URL is an arXiv `/pdf/`
/// link carrying an explicit version — the bucket keys objects by `<id>v<n>.pdf`
/// with no version-less alias — so an unmappable URL is left to the arXiv host.
///
/// New-style `…/pdf/2204.12985v4` → `…/arxiv/arxiv/pdf/2204/2204.12985v4.pdf`.
/// Old-style `…/pdf/hep-th/9901001v1` → `…/arxiv/hep-th/pdf/9901/9901001v1.pdf`.
pub(crate) fn gcs_pdf_url(pdf_url: &str) -> Option<String> {
    // Only an arXiv-hosted URL may be mapped onto the mirror; otherwise the proxy
    // would fetch an attacker-chosen storage.googleapis.com object. A rejected host
    // yields None, so the caller falls back to the host-guarded arXiv fetch.
    http::assert_host_allowed(pdf_url, http::ARXIV_HOSTS).ok()?;
    let after = pdf_url.split_once("/pdf/")?.1;
    let after = after
        .split(['?', '#'])
        .next()
        .unwrap_or(after)
        .trim_matches('/');
    let (archive_path, last) = after.rsplit_once('/').unwrap_or(("", after));
    // Tolerate the canonical `.pdf` suffix on the object id.
    let last = last.strip_suffix(".pdf").unwrap_or(last);
    // The bucket has no version-less object, so an explicit trailing v<digits> is
    // required; without one, defer to the arXiv host.
    let vpos = last.rfind('v')?;
    let (base, vdigits) = (&last[..vpos], &last[vpos + 1..]);
    if base.is_empty() || vdigits.is_empty() || !vdigits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (archive, yymm) = match base.split_once('.') {
        // New-style "YYMM.NNNNN": the archive bucket is literally "arxiv".
        Some((yymm, _)) => ("arxiv", yymm),
        // Old-style "<archive>/NNNNNNN": the archive is a single path segment —
        // reject a multi-segment or `..` path so it can't escape the bucket.
        None => {
            if archive_path.contains('/') || archive_path.contains("..") {
                return None;
            }
            (archive_path, base.get(..4)?)
        }
    };
    if archive.is_empty() || yymm.len() != 4 || !yymm.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "https://storage.googleapis.com/arxiv-dataset/arxiv/{archive}/pdf/{yymm}/{last}.pdf"
    ))
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

/// Append `text` to `out` with each line's TeX comment removed.
fn push_stripped_lines(out: &mut String, text: &str) {
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(strip_line_comment(line));
    }
}

/// Remove TeX comments then boilerplate commands. Port of `_strip_tex_noise`.
pub fn strip_tex_noise(text: &str) -> String {
    let mut no_comments = String::with_capacity(text.len());
    push_stripped_lines(&mut no_comments, text);
    strip_commands(&no_comments)
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

/// Ceiling on `.tex` bytes retained for the FTS index, read out of one tarball
/// through a shrinking allowance rather than trusting the archive's own sizes.
/// Bytes from skipped (non-`.tex` or unsafe) members are bounded separately by
/// `MAX_DECOMPRESSED_BYTES` on the shared reader.
const MAX_TEX_BYTES: u64 = 16 * 1024 * 1024;

/// Ceiling on bytes pulled through the gzip decoder for the whole tarball,
/// including bytes from members skipped before reaching `MAX_TEX_BYTES`.
const MAX_DECOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;

/// Extract TeX source from a `.tar.gz` at `tarpath`, returning concatenated,
/// noise-stripped plain text. `.tex` members only, root files before nested
/// (stable sort by `/` depth), unsafe paths skipped. Any tar/io error -> `""`.
/// Every safe `.tex` member is read in full (bounded only by the archive-level
/// `MAX_DECOMPRESSED_BYTES` cap) before the root-first sort runs; `MAX_TEX_BYTES`
/// is then applied to the sorted list.
/// Port of `extract_source` (we read members straight from the stream rather
/// than extracting to a temp dir — the path-guard makes a temp dir unneeded).
pub fn extract_source(tarpath: &Path) -> String {
    extract_source_inner(tarpath).unwrap_or_default()
}

fn extract_source_inner(tarpath: &Path) -> Result<String> {
    extract_capped(tarpath, MAX_DECOMPRESSED_BYTES, MAX_TEX_BYTES)
}

/// `extract_source_inner` with both ceilings injected, so the bomb guards can be
/// exercised at kilobyte scale instead of allocating the real 256 MiB.
fn extract_capped(tarpath: &Path, max_decompressed: u64, max_tex: u64) -> Result<String> {
    let file = std::fs::File::open(tarpath)
        .map_err(|e| CoreError::Internal(format!("open {tarpath:?}: {e}")))?;
    let mut archive = Archive::new(GzDecoder::new(file).take(max_decompressed));

    let mut tex: Vec<(String, Vec<u8>)> = Vec::new();
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
        // Bounded only by the archive-level `MAX_DECOMPRESSED_BYTES` cap; the
        // MAX_TEX_BYTES budget is applied below, after the root-first sort.
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| CoreError::Internal(format!("read member {name:?}: {e}")))?;
        tex.push((name, bytes));
    }

    if tex.is_empty() {
        return Ok(String::new());
    }
    tex.sort_by_key(|(name, _)| name.matches('/').count()); // stable: root first

    // Comment-strip each member straight into one pre-sized buffer (with the
    // "\n\n" join separators inline) instead of materializing per-member owned
    // strings, the joined text, and the comment-stripped rejoin — this path can
    // run to MAX_TEX_BYTES, so each avoided copy is up to 16 MiB.
    let total: u64 = tex.iter().map(|(_, b)| b.len() as u64).sum();
    let mut clean = String::with_capacity(total.min(max_tex) as usize + 2 * tex.len());
    let mut remaining = max_tex;
    let mut first = true;
    for (_, bytes) in &tex {
        if remaining == 0 {
            break;
        }
        let take = (bytes.len() as u64).min(remaining) as usize;
        remaining -= take as u64;
        if !first {
            clean.push_str("\n\n");
        }
        first = false;
        push_stripped_lines(&mut clean, &String::from_utf8_lossy(&bytes[..take]));
    }
    Ok(strip_commands(&clean))
}

// ---------------------------------------------------------------------------
// PDF / source download  (async, streamed; integration-tested via http later)
// ---------------------------------------------------------------------------

/// Ceiling on a streamed download. The body is written as it arrives, so without
/// a running total a hostile or merely enormous response would fill the disk;
/// `Content-Length` is not trusted for this, only bytes actually written.
/// Matches `sources::download::MAX_PDF_BYTES` (200 MiB, the Python spec's cap).
const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;

/// Stream `arxiv_get(url)` to `dest_dir/<filename>` atomically (tmp -> rename),
/// refusing a body that grows past `max_bytes`. The partial file is best-effort
/// removed on failure (a failed cleanup delete is logged, not fatal).
async fn stream_to(
    url: &str,
    dest_dir: &Path,
    filename: &str,
    data_dir: &Path,
    max_bytes: u64,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dest_dir)
        .await
        .map_err(|e| CoreError::Internal(format!("mkdir {dest_dir:?}: {e}")))?;

    let dest = dest_dir.join(filename);
    let tmp = dest_dir.join(format!(".{filename}.{}.part", uuid::Uuid::new_v4()));
    let out = stream_to_tmp(url, &tmp, &dest, data_dir, max_bytes).await;
    if out.is_err() {
        if let Err(e) = tokio::fs::remove_file(&tmp).await {
            tracing::warn!("failed to remove partial download {tmp:?}: {e}");
        }
    }
    out
}

async fn stream_to_tmp(
    url: &str,
    tmp: &Path,
    dest: &Path,
    data_dir: &Path,
    max_bytes: u64,
) -> Result<PathBuf> {
    let mut resp = http::arxiv_get(url, data_dir).await?;
    if !resp.status().is_success() {
        return Err(CoreError::Upstream(format!(
            "GET {url:?} -> HTTP {}",
            resp.status().as_u16()
        )));
    }

    let mut file = tokio::fs::File::create(tmp)
        .await
        .map_err(|e| CoreError::Internal(format!("create {tmp:?}: {e}")))?;
    // chunk() streams the body without pulling in a `futures` Stream dependency.
    let mut written: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CoreError::Upstream(format!("stream {url:?}: {e}")))?
    {
        written += chunk.len() as u64;
        if written > max_bytes {
            return Err(CoreError::Upstream(format!(
                "{url:?} exceeds the {max_bytes} byte download limit"
            )));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| CoreError::Internal(format!("write {tmp:?}: {e}")))?;
    }
    file.flush()
        .await
        .map_err(|e| CoreError::Internal(format!("flush {tmp:?}: {e}")))?;
    drop(file);
    tokio::fs::rename(tmp, dest)
        .await
        .map_err(|e| CoreError::Internal(format!("rename -> {dest:?}: {e}")))?;
    Ok(dest.to_path_buf())
}

/// Download a paper's PDF into `dest_dir`. `pdf_url` is the arXiv PDF URL
/// (`arxiv_get` rewrites the host to `export.arxiv.org`). Returns the path.
pub async fn download_pdf(pdf_url: &str, dest_dir: &Path, data_dir: &Path) -> Result<PathBuf> {
    let filename = default_filename(pdf_url, "pdf");
    stream_to(pdf_url, dest_dir, &filename, data_dir, MAX_DOWNLOAD_BYTES).await
}

/// Download a paper's TeX-source tarball into `dest_dir`. Derived from the PDF
/// URL by `/pdf/`->`/src/`. Returns the path written.
pub async fn download_source(pdf_url: &str, dest_dir: &Path, data_dir: &Path) -> Result<PathBuf> {
    let src_url = pdf_to_src(pdf_url);
    let filename = default_filename(pdf_url, "tar.gz");
    stream_to(&src_url, dest_dir, &filename, data_dir, MAX_DOWNLOAD_BYTES).await
}

/// Fetch a paper's arXiv TeX source and return the extracted, noise-stripped
/// text — the write half of full-text search, which until now had no caller.
/// The tarball lands in a temp dir that is dropped (and deleted) before the text
/// is returned.
///
/// An empty string means the tarball held no usable `.tex` (arXiv serves PDF-only
/// submissions from the same `/src/` path); callers treat that as "no full text
/// available", not as an error.
pub async fn fetch_source_text(pdf_url: &str, data_dir: &Path) -> Result<String> {
    let scratch = tempfile::tempdir()
        .map_err(|e| CoreError::Internal(format!("create temp dir for TeX source: {e}")))?;
    let tarball = download_source(pdf_url, scratch.path(), data_dir).await?;
    // Gunzip + tar walk is CPU-bound and can run to MAX_TEX_BYTES, so keep it off
    // the async worker. `scratch` is moved in and dropped here, deleting the tarball.
    tokio::task::spawn_blocking(move || {
        let text = extract_source(&tarball);
        drop(scratch);
        text
    })
    .await
    .map_err(|e| CoreError::Internal(format!("extract TeX source: {e}")))
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

    #[test]
    fn gcs_pdf_url_maps_new_and_old_style() {
        assert_eq!(
            gcs_pdf_url("http://arxiv.org/pdf/2204.12985v4").as_deref(),
            Some("https://storage.googleapis.com/arxiv-dataset/arxiv/arxiv/pdf/2204/2204.12985v4.pdf")
        );
        assert_eq!(
            gcs_pdf_url("https://arxiv.org/pdf/hep-th/9901001v1").as_deref(),
            Some(
                "https://storage.googleapis.com/arxiv-dataset/arxiv/hep-th/pdf/9901/9901001v1.pdf"
            )
        );
        // Canonical `.pdf`-suffixed form maps to the same object.
        assert_eq!(
            gcs_pdf_url("https://arxiv.org/pdf/2204.12985v4.pdf").as_deref(),
            Some("https://storage.googleapis.com/arxiv-dataset/arxiv/arxiv/pdf/2204/2204.12985v4.pdf")
        );
        // No explicit version -> unmappable -> caller falls back to the arXiv host.
        assert_eq!(gcs_pdf_url("http://arxiv.org/pdf/2204.12985"), None);
        // Non-arXiv host must NOT be mapped onto the mirror (SSRF host guard).
        assert_eq!(gcs_pdf_url("https://evil.com/pdf/9901001v1"), None);
        // A traversing path can't escape the arxiv-dataset bucket.
        assert_eq!(
            gcs_pdf_url("https://arxiv.org/pdf/a/../../x/9901001v1"),
            None
        );
        // Not an arXiv /pdf/ link.
        assert_eq!(gcs_pdf_url("https://example.com/foo.pdf"), None);
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
        assert_eq!(out, "ROOT\n\nNESTED");
    }

    #[test]
    fn extract_member_boundary_comment_does_not_eat_next_member() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tarball(
            dir.path(),
            "src.tar.gz",
            &[("main.tex", "ROOT % trailing"), ("z.tex", "NEXT")],
        );
        assert_eq!(extract_source(&p), "ROOT \n\nNEXT");
    }

    #[test]
    fn extract_empty_when_no_tex() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_tarball(dir.path(), "src.tar.gz", &[("readme.txt", "no tex here")]);
        assert_eq!(extract_source(&p), "");
    }

    // The two ceilings are exercised through `extract_capped` at kilobyte scale.
    // Driving them through the real 16 MiB / 256 MiB constants would allocate a
    // 257 MiB String per run for the same assertions.

    #[test]
    fn extract_stops_at_the_tex_byte_cap() {
        // Two members, each already at the cap: the allowance carries across
        // members rather than resetting per member.
        let dir = tempfile::tempdir().unwrap();
        let big = "a".repeat(4096);
        let p = write_tarball(
            dir.path(),
            "big.tar.gz",
            &[("main.tex", &big), ("more.tex", &big)],
        );
        let out = extract_capped(&p, 1 << 20, 4096).unwrap();
        assert!(
            out.len() <= 4096,
            "extracted {} bytes, cap is 4096",
            out.len()
        );
        // Truncated, not discarded.
        assert!(!out.is_empty());
    }

    #[test]
    fn extract_bounds_decompressed_bytes_against_zip_bomb() {
        // A member that inflates well past the decompression allowance, and a
        // non-.tex member ahead of it — the tar crate inflates skipped members to
        // seek past them, so the guard has to sit on the shared reader.
        let dir = tempfile::tempdir().unwrap();
        let huge = "a".repeat(4 * 1024 * 1024);
        let p = write_tarball(
            dir.path(),
            "bomb.tar.gz",
            &[("padding.bin", &huge), ("main.tex", &huge)],
        );
        // Compresses to a few KB, so the cap — not the file size — is what binds.
        assert!(std::fs::metadata(&p).unwrap().len() < 64 * 1024);
        // The reader runs dry mid-skip, so the tar walk aborts instead of
        // inflating the member: the allowance, not the archive, decides.
        let err = extract_capped(&p, 64 * 1024, 4 * 1024 * 1024).unwrap_err();
        assert!(
            err.to_string().contains("tar entry"),
            "expected the capped reader to cut the tar walk short, got {err}"
        );
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

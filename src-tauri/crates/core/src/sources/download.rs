//! download — SSRF-safe PDF downloader. Rust port of `service/files.py::download_pdf`
//! (scheme allowlist, host-resolves-to-public, redirect re-check per hop, content-type +
//! size cap, atomic tmp→dest rename). Plan §5.4.
//!
//! DI: the caller (`service::files::download_pdf`) computes the managed dest path under the
//! injected `pdf_dir` and passes it in — this module never reads config. Tests pass a
//! `tempfile::tempdir()` dest.

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::Url;

use crate::error::{CoreError, Result};

/// Deletes its path on drop unless `disarm`ed — so any error after the temp file is created
/// removes it (mirrors Python's `tmp.unlink(missing_ok=True)` on failure).
struct TmpGuard(Option<PathBuf>);
impl TmpGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}
impl Drop for TmpGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Matches Python `_MAX_PDF_BYTES = 200 * 1024 * 1024` (the spec's "~100MB" cap; the live
/// value is 200 MB — port the real number).
const MAX_PDF_BYTES: u64 = 200 * 1024 * 1024;
/// Python `_ALLOWED_CONTENT_TYPES`. An empty/absent Content-Type is allowed (Python only
/// rejects a *present* type that isn't one of these).
const ALLOWED_CONTENT_TYPES: &[&str] = &["application/pdf", "application/octet-stream"];
const MAX_REDIRECTS: u32 = 10;

/// Download `url` into `dest` (which already encodes the DI'd managed pdf_dir). Returns the
/// dest path. Idempotent: if `dest` already exists it is returned without a network call
/// (mirrors Python's `if dest.exists(): return str(dest)`).
pub async fn download_pdf(dest: &Path, url: &str) -> Result<PathBuf> {
    if dest.exists() {
        return Ok(dest.to_path_buf());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Internal(format!("could not create pdf dir: {e}")))?;
    }
    let client = build_client()?;
    fetch_to_dest(&client, url, dest, MAX_PDF_BYTES, &host_is_public).await
}

/// A redirect-DISABLED client so the SSRF guard re-runs on every hop's URL (an auto-following
/// client would chase a 302→169.254.169.254 before we could veto it).
fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // Dedicated no-redirect client (manual per-hop SSRF re-check), but share http's UA.
        .user_agent(crate::sources::http::USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| CoreError::Internal(format!("http client build failed: {e}")))
}

/// Core fetch: enforce scheme + `host_ok` on the initial URL and every redirect hop, then
/// stream the final body to `dest` under `max_bytes` with an atomic rename. `host_ok` and
/// `max_bytes` are injected so tests can drive a 127.0.0.1 mock (which the real public-IP
/// guard would reject) and exercise the size cap without a 200 MB body.
async fn fetch_to_dest(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    max_bytes: u64,
    // `+ Sync` so `&dyn Fn` is `Send`: the rmcp `#[tool]` macro boxes the download_pdf future as
    // `dyn Future + Send`, which requires every value held across an await (this guard) to be Send.
    host_ok: &(dyn Fn(&Url) -> bool + Sync),
) -> Result<PathBuf> {
    let mut current =
        Url::parse(url).map_err(|e| CoreError::BadRequest(format!("Invalid URL: {e}")))?;
    let mut hops = 0u32;

    let resp = loop {
        let scheme = current.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(CoreError::Validation(format!(
                "Unsafe URL scheme '{scheme}'. Only http/https are allowed."
            )));
        }
        if !host_ok(&current) {
            return Err(CoreError::Validation(format!(
                "URL host '{}' resolves to a disallowed network range.",
                current.host_str().unwrap_or("")
            )));
        }
        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| CoreError::Upstream(format!("download failed: {e}")))?;

        if resp.status().is_redirection() {
            hops += 1;
            if hops > MAX_REDIRECTS {
                return Err(CoreError::Upstream("too many redirects".into()));
            }
            let loc = resp
                .headers()
                .get(LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| CoreError::Upstream("redirect without Location".into()))?;
            current = current
                .join(loc)
                .map_err(|e| CoreError::BadRequest(format!("bad redirect target: {e}")))?;
            continue;
        }
        break resp;
    };

    // Non-2xx is an error, not a body: Python urllib raises HTTPError on any non-2xx, so a 4xx/5xx
    // error page served with a pdf/octet content-type must not be saved as a corrupt "PDF".
    if !resp.status().is_success() {
        return Err(CoreError::Upstream(format!(
            "download failed: HTTP {}",
            resp.status()
        )));
    }

    // Content-Type: reject a present type that isn't a PDF/octet-stream (empty → allowed).
    let ct = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let ct_main = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !ct_main.is_empty() && !ALLOWED_CONTENT_TYPES.contains(&ct_main.as_str()) {
        return Err(CoreError::Validation(format!(
            "Unexpected Content-Type '{ct_main}'; expected PDF."
        )));
    }

    // Declared Content-Length over the cap → reject before streaming a byte.
    if let Some(declared) = resp
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        if declared > max_bytes {
            return Err(CoreError::Validation(format!(
                "File too large ({declared} bytes; limit {max_bytes})."
            )));
        }
    }

    // Stream into a sibling temp file under dest's dir, then atomically rename. Any error
    // after creation drops the guard → temp removed (Python's tmp.unlink on failure).
    let parent = dest
        .parent()
        .ok_or_else(|| CoreError::Internal("dest has no parent dir".into()))?;
    let tmp_path = parent.join(tmp_name());
    let mut file = std::fs::File::create(&tmp_path)
        .map_err(|e| CoreError::Internal(format!("tmp create failed: {e}")))?;
    let mut guard = TmpGuard(Some(tmp_path.clone()));

    let mut total: u64 = 0;
    let mut resp = resp;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| CoreError::Upstream(format!("download read failed: {e}")))?
    {
        total += chunk.len() as u64;
        if total > max_bytes {
            return Err(CoreError::Validation(format!(
                "Download exceeded {max_bytes} byte limit."
            )));
        }
        file.write_all(&chunk)
            .map_err(|e| CoreError::Internal(format!("write failed: {e}")))?;
    }
    file.flush()
        .map_err(|e| CoreError::Internal(format!("flush failed: {e}")))?;
    drop(file); // close before rename (matters on Windows; harmless on unix)
    std::fs::rename(&tmp_path, dest)
        .map_err(|e| CoreError::Internal(format!("atomic rename failed: {e}")))?;
    guard.disarm();
    Ok(dest.to_path_buf())
}

/// Unique sibling temp filename. pid + monotonic-ish nanos + a process-local counter — no rng
/// crate needed, collision-proof for one process's downloads.
fn tmp_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".dl-{}-{}-{}.tmp", std::process::id(), nanos, n)
}

/// True iff `url`'s host resolves *only* to public addresses. IP-literal hosts are checked
/// directly (no DNS); domain hosts resolve via blocking `getaddrinfo` and ALL results must be
/// public (any private/loopback/etc → reject), matching Python's `_is_safe_host`.
// DNS-rebind between check and connect is the same known residual gap the Python
// version documents.
fn host_is_public(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    // host_str keeps brackets on IPv6 literals ("[::1]"); strip them before parsing.
    let bare = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return is_public_addr(ip);
    }
    match (host, 0u16).to_socket_addrs() {
        Ok(addrs) => {
            let mut any = false;
            for a in addrs {
                any = true;
                if !is_public_addr(a.ip()) {
                    return false;
                }
            }
            any // no addresses == unresolved == unsafe
        }
        Err(_) => false,
    }
}

/// SECURITY CORE: reject any non-public address (private, loopback, link-local, unique-local,
/// multicast, unspecified, CGNAT-shared, 0.0.0.0/8). Mirrors Python's `ipaddress` is_* checks.
fn is_public_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            // An embedded IPv4 (::ffff:a.b.c.d) is reachable as that v4 — judge it as v4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            let s = v6.segments();
            let seg0 = s[0];
            let unique_local = (seg0 & 0xfe00) == 0xfc00; // fc00::/7
            let link_local = (seg0 & 0xffc0) == 0xfe80; // fe80::/10
                                                        // Reserved ranges Python rejects via addr.is_reserved.
            let documentation = seg0 == 0x2001 && s[1] == 0x0db8; // 2001:db8::/32
            let discard = seg0 == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0; // 100::/64
            let nat64 = seg0 == 0x0064
                && s[1] == 0xff9b
                && s[2] == 0
                && s[3] == 0
                && s[4] == 0
                && s[5] == 0; // 64:ff9b::/96
            !(unique_local || link_local || documentation || discard || nat64)
        }
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    let shared = o[0] == 100 && (o[1] & 0xc0) == 0x40; // 100.64.0.0/10 CGNAT
                                                       // Reserved ranges Python rejects via addr.is_reserved that the std is_* checks miss.
    let reserved = o[0] >= 240 // 240.0.0.0/4 (reserved/future)
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // 198.18.0.0/15 (benchmarking)
        || (o[0] == 192 && o[1] == 0 && o[2] == 0); // 192.0.0.0/24 (IETF protocol assignments)
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_documentation()
        || v4.is_unspecified()
        || v4.is_multicast()
        || shared
        || reserved
        || o[0] == 0) // 0.0.0.0/8
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }

    #[test]
    fn is_public_addr_classifies_ssrf_vectors() {
        // Public → allowed.
        assert!(is_public_addr(v4("8.8.8.8")));
        assert!(is_public_addr(v4("1.1.1.1")));
        assert!(is_public_addr(IpAddr::V6(
            "2606:4700:4700::1111".parse().unwrap()
        )));

        // Non-public → rejected (the SSRF blocklist).
        for bad in [
            "10.0.0.1",        // private
            "192.168.1.1",     // private
            "172.16.0.1",      // private
            "169.254.169.254", // link-local (cloud metadata)
            "127.0.0.1",       // loopback
            "0.0.0.0",         // unspecified / 0/8
            "100.64.0.1",      // CGNAT shared
            "240.0.0.1",       // 240.0.0.0/4 reserved
            "255.0.0.1",       // 240/4 upper end
            "198.18.0.1",      // 198.18.0.0/15 benchmarking
            "198.19.255.1",    // 198.18.0.0/15 upper half
            "192.0.0.1",       // 192.0.0.0/24 IETF protocol assignments
        ] {
            assert!(!is_public_addr(v4(bad)), "{bad} must be rejected");
        }
        // Reserved ranges adjacent to the blocked ones stay public (no over-blocking).
        assert!(is_public_addr(v4("198.17.0.1"))); // just below 198.18.0.0/15
        assert!(is_public_addr(v4("198.20.0.1"))); // just above 198.18.0.0/15
        assert!(is_public_addr(v4("192.0.1.1"))); // just above 192.0.0.0/24
        assert!(!is_public_addr(IpAddr::V6("2001:db8::1".parse().unwrap()))); // documentation
        assert!(!is_public_addr(IpAddr::V6("64:ff9b::1".parse().unwrap()))); // NAT64
        assert!(!is_public_addr(IpAddr::V6(Ipv6Addr::LOCALHOST))); // ::1
        assert!(!is_public_addr(IpAddr::V6("fe80::1".parse().unwrap()))); // link-local
        assert!(!is_public_addr(IpAddr::V6("fc00::1".parse().unwrap()))); // unique-local
        assert!(!is_public_addr(IpAddr::V6("ff02::1".parse().unwrap()))); // multicast
                                                                          // IPv4-mapped loopback must be judged as the embedded v4.
        assert!(!is_public_addr(IpAddr::V6(
            "::ffff:127.0.0.1".parse().unwrap()
        )));
    }

    #[tokio::test]
    async fn rejects_non_http_scheme_offline() {
        let client = build_client().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let dest = dest.path().join("out.pdf");
        let err = fetch_to_dest(&client, "file:///etc/passwd", &dest, MAX_PDF_BYTES, &|_| {
            true
        })
        .await
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(m) if m.contains("scheme")));
    }

    #[tokio::test]
    async fn rejects_private_host_offline() {
        // Real guard, IP literals → no DNS, no network. Each must be vetoed before any send().
        let client = build_client().unwrap();
        let dest = tempfile::tempdir().unwrap();
        for url in [
            "http://127.0.0.1/x.pdf",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/x.pdf",
            "http://[fc00::1]/x.pdf",
        ] {
            let out = dest.path().join("out.pdf");
            let err = fetch_to_dest(&client, url, &out, MAX_PDF_BYTES, &host_is_public)
                .await
                .unwrap_err();
            assert!(
                matches!(err, CoreError::Validation(m) if m.contains("disallowed")),
                "{url} should be rejected as disallowed"
            );
        }
    }

    #[tokio::test]
    async fn downloads_pdf_and_atomically_writes() {
        let server = MockServer::start().await;
        let body = b"%PDF-1.7 hello".to_vec();
        Mock::given(method("GET"))
            .and(path("/paper.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(body.clone()),
            )
            .mount(&server)
            .await;

        let client = build_client().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("2204.00001v1.pdf");
        let url = format!("{}/paper.pdf", server.uri());
        // Mock binds 127.0.0.1, which the real guard rejects → permissive host_ok for the body path.
        let out = fetch_to_dest(&client, &url, &dest, MAX_PDF_BYTES, &|_| true)
            .await
            .unwrap();
        assert_eq!(out, dest);
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[tokio::test]
    async fn rejects_wrong_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<html>nope</html>"),
            )
            .mount(&server)
            .await;
        let client = build_client().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("o.pdf");
        let err = fetch_to_dest(
            &client,
            &format!("{}/x", server.uri()),
            &dest,
            MAX_PDF_BYTES,
            &|_| true,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(m) if m.contains("Content-Type")));
        assert!(
            !dest.exists(),
            "no temp file should survive a rejected download"
        );
    }

    #[tokio::test]
    async fn enforces_size_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_bytes(vec![0u8; 64]),
            )
            .mount(&server)
            .await;
        let client = build_client().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("o.pdf");
        // max_bytes injected at 4 → the 64-byte body is over the cap.
        let err = fetch_to_dest(&client, &format!("{}/big", server.uri()), &dest, 4, &|_| {
            true
        })
        .await
        .unwrap_err();
        assert!(
            matches!(err, CoreError::Validation(m) if m.contains("too large") || m.contains("exceeded"))
        );
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn rejects_non_2xx_even_with_pdf_content_type() {
        // A 404 error page served as application/pdf must error, not be saved as a corrupt PDF.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.pdf"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("not here"),
            )
            .mount(&server)
            .await;
        let client = build_client().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("o.pdf");
        let err = fetch_to_dest(
            &client,
            &format!("{}/missing.pdf", server.uri()),
            &dest,
            MAX_PDF_BYTES,
            &|_| true,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, CoreError::Upstream(ref m) if m.contains("HTTP 404")),
            "{err}"
        );
        assert!(
            !dest.exists(),
            "a non-2xx response must leave no file at dest"
        );
    }

    #[tokio::test]
    async fn redirect_to_private_host_is_rechecked() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redir"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data"),
            )
            .mount(&server)
            .await;
        let client = build_client().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("o.pdf");
        // host_ok allows only the mock's loopback host; the redirect target must hit the real
        // public-IP guard and be vetoed → proves the check re-runs per hop, not just initially.
        let host_ok = |u: &Url| u.host_str() == Some("127.0.0.1") || host_is_public(u);
        let err = fetch_to_dest(
            &client,
            &format!("{}/redir", server.uri()),
            &dest,
            MAX_PDF_BYTES,
            &host_ok,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CoreError::Validation(m) if m.contains("disallowed")));
    }
}

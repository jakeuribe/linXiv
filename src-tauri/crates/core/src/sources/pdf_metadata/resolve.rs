//! Network enrichment orchestration — `resolve_pdf_metadata` and the
//! PDF-metadata-first identity/enrichment policy over arXiv/DOI/CrossRef.

use std::path::Path;

use chrono::{NaiveDate, Utc};

use crate::error::Result;
use crate::models::PaperMetadata;
use crate::sources::{arxiv, crossref, doi_resolve};

use super::extract::Extracted;
use super::identity::pdf_source_id;
use super::scan::title_similarity;
use super::worker::extract_pdf_metadata_isolated;

// ---------------------------------------------------------------------------
// resolve_pdf_metadata — the seam `import_pdf` injects
// ---------------------------------------------------------------------------

/// Title + at least one real author name both present (already past the junk
/// filters) — enough to call the PDF's own metadata the record, no network. Year
/// is not required. Checks `split_authors` so stray-separator fields like `"; ; "` fail.
fn pdf_metadata_is_sufficient(raw: &Extracted) -> bool {
    raw.title.as_deref().is_some_and(|t| !t.trim().is_empty())
        && raw
            .authors
            .as_deref()
            .is_some_and(|a| !split_authors(a).is_empty())
}

/// Split a PDF Info-dict Author string into names. `;` is unambiguous and wins.
/// Otherwise comma-split with one repair pass: a bare token with no whitespace
/// ("Smith") is re-merged with the next ("John") into "Smith, John", so "Last,
/// First" exports survive while "Alice Smith, Bob Jones" passes untouched.
/// ponytail: an ODD-length comma list of bare surnames only still misreads the
/// trailing pair as one merged author; true name-parsing needed to fully fix.
fn split_authors(raw: &str) -> Vec<String> {
    if raw.contains(';') {
        return raw
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
    }
    let parts: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut authors = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        if !parts[i].contains(char::is_whitespace) && i + 1 < parts.len() {
            authors.push(format!("{}, {}", parts[i], parts[i + 1]));
            i += 2;
        } else {
            authors.push(parts[i].to_string());
            i += 1;
        }
    }
    authors
}

/// A new-style arXiv id (`YYMM.NNNNN`) encodes submission year+month in its first
/// 4 digits (scheme started 0704, so YY -> 20YY); day unknown, lands on the 1st.
fn arxiv_id_year_month(id: &str) -> Option<NaiveDate> {
    let b = id.as_bytes();
    if b.len() < 4 || !b[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let yy: i32 = std::str::from_utf8(&b[..2]).ok()?.parse().ok()?;
    let mm: u32 = std::str::from_utf8(&b[2..4]).ok()?.parse().ok()?;
    NaiveDate::from_ymd_opt(2000 + yy, mm, 1)
}

/// `Extracted` -> the local-id partial `PaperMetadata` (title/authors/year/doi only
/// — the PDF carries nothing else; the page-scanned `doi` is PDF-derived, so kept).
fn partial_meta_from_raw(local_id: String, raw: &Extracted) -> PaperMetadata {
    let authors: Vec<String> = raw
        .authors
        .as_deref()
        .map(split_authors)
        .unwrap_or_default();

    // Prefer the PDF's own CreationDate/ModificationDate year; failing that, an
    // arXiv id's embedded YYMM is still PDF-derived (no network); only then today.
    let published = raw
        .year
        .and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1))
        .or_else(|| raw.arxiv_id.as_deref().and_then(arxiv_id_year_month))
        .unwrap_or_else(|| Utc::now().date_naive());

    PaperMetadata {
        source_id: local_id,
        version: 1,
        title: raw.title.clone().unwrap_or_default(),
        authors,
        published,
        updated: None,
        summary: String::new(),
        category: None,
        categories: None,
        doi: raw.doi.clone(),
        journal_ref: None,
        comment: None,
        url: None,
        tags: None,
        source: Some("pdf".into()),
        author_orcids: None,
    }
}

/// PDF-metadata-first resolution. Returns `(meta, external_identity)`: `meta`
/// always carries the `local:<sha256>` id; `external_identity` is the upstream
/// `(source_id, version)` when known. When the PDF's title+authors suffice, the
/// FIELDS never come from the network; a text-scanned arXiv id/DOI is only a
/// CANDIDATE identity (page 1 can cite someone else's paper), so with
/// `verify_identity` on this makes ONE lookup (arXiv id first, else DOI) and
/// adopts it only on a >= 0.5 title-similarity match — off, a candidate is never
/// adopted and no lookup is made. Insufficient PDF metadata falls through to
/// full arXiv/DOI/CrossRef enrichment (NOT gated), else the partial record.
async fn resolve_from_extracted(
    local_id: String,
    raw: Extracted,
    data_dir: &Path,
    mailto: &str,
    verify_identity: bool,
) -> Result<(PaperMetadata, Option<(String, i64)>)> {
    if pdf_metadata_is_sufficient(&raw) {
        let external = if verify_identity {
            match identity_candidate(&raw) {
                IdentityCandidate::Arxiv(id) => verify_arxiv_identity(id, &raw, data_dir).await,
                IdentityCandidate::Doi(doi) => {
                    verify_doi_identity(doi, &raw, data_dir, mailto).await
                }
                IdentityCandidate::None => None,
            }
        } else {
            None
        };
        tracing::debug!(
            local_id = %local_id,
            external = ?external,
            verify_identity,
            "pdf metadata sufficient: resolved main fields from the PDF, no field-level network enrichment"
        );
        return Ok((partial_meta_from_raw(local_id, &raw), external));
    }
    tracing::debug!(
        local_id = %local_id,
        has_title = raw.title.is_some(),
        has_authors = raw.authors.is_some(),
        "pdf metadata insufficient: moving on to arXiv/DOI/CrossRef enrichment"
    );

    // PDF metadata alone wasn't enough — move on to the other things. Enrich
    // from an upstream record when an arXiv id / DOI / title matches.
    // external_identity = (enriched.source_id, version) so the importer can
    // dedupe against an existing root; the returned meta keeps the local id.
    if let Some(enriched) = enrich_external(&raw, data_dir, mailto).await {
        let external = (!enriched.source_id.is_empty())
            .then(|| (enriched.source_id.clone(), enriched.version));
        let meta = PaperMetadata {
            source_id: local_id,
            version: 1,
            title: if enriched.title.is_empty() {
                raw.title.clone().unwrap_or_default()
            } else {
                enriched.title
            },
            authors: enriched.authors,
            published: enriched.published,
            updated: None,
            summary: enriched.summary,
            category: enriched.category,
            categories: None,
            doi: enriched.doi,
            journal_ref: None,
            comment: None,
            url: enriched.url,
            tags: None,
            source: Some("pdf".into()),
            author_orcids: None,
        };
        return Ok((meta, external));
    }

    // No enrichment either: partial record from whatever extraction did produce.
    Ok((partial_meta_from_raw(local_id, &raw), None))
}

/// At most one identity candidate — arXiv id if found, else DOI. Deliberately
/// NOT "arXiv, then DOI on failure": that would turn a failed verification into
/// a second round-trip, breaking the one-lookup promise. A citation-only arXiv
/// id costs the DOI dedupe chance in that case — accepted.
enum IdentityCandidate<'a> {
    Arxiv(&'a str),
    Doi(&'a str),
    None,
}

fn identity_candidate(raw: &Extracted) -> IdentityCandidate<'_> {
    match (raw.arxiv_id.as_deref(), raw.doi.as_deref()) {
        (Some(id), _) => IdentityCandidate::Arxiv(id),
        (None, Some(doi)) => IdentityCandidate::Doi(doi),
        (None, None) => IdentityCandidate::None,
    }
}

/// The pure decision behind the verify_* fns: does `fetched`'s title look like
/// `raw_title` (same 0.5 Jaccard bar as `try_crossref_title`)? This is what keeps
/// a citation or wrong id from being silently adopted as identity.
fn identity_if_title_matches(raw_title: &str, fetched: PaperMetadata) -> Option<(String, i64)> {
    (title_similarity(raw_title, &fetched.title) >= 0.5)
        .then_some((fetched.source_id, fetched.version))
}

/// A page-1-scanned arXiv id is only a CANDIDATE (it may be a citation). `None`
/// on any fetch failure or title mismatch — never a wrong identity silently adopted.
async fn verify_arxiv_identity(
    id: &str,
    raw: &Extracted,
    data_dir: &Path,
) -> Option<(String, i64)> {
    let raw_title = raw.title.as_deref()?;
    let fetched = arxiv::fetch_by_id(id, data_dir).await.ok()?;
    identity_if_title_matches(raw_title, fetched)
}

/// Same guard for a page-1-scanned DOI; only reached when no arXiv id was found.
/// `resolve_doi` is the one call that can map a bare DOI to a source_id at all.
async fn verify_doi_identity(
    doi: &str,
    raw: &Extracted,
    data_dir: &Path,
    mailto: &str,
) -> Option<(String, i64)> {
    let raw_title = raw.title.as_deref()?;
    let fetched = doi_resolve::resolve_doi(doi, data_dir, mailto).await.ok()?;
    identity_if_title_matches(raw_title, fetched)
}

/// PDF bytes -> `Extracted` -> `resolve_from_extracted` (split so tests drive a
/// synthetic `Extracted`). `verify_identity` is read by the caller from
/// `UserSettings::pdf_import_verify_identity_enabled` — this module never reads config.
pub async fn resolve_pdf_metadata(
    bytes: &[u8],
    data_dir: &Path,
    mailto: &str,
    verify_identity: bool,
) -> Result<(PaperMetadata, Option<(String, i64)>)> {
    // Blocking work (child-process wait, or native FFI on the in-process
    // fallback) — off the executor onto the blocking pool.
    let owned = bytes.to_vec();
    let raw = tokio::task::spawn_blocking(move || extract_pdf_metadata_isolated(&owned))
        .await
        .unwrap_or_default();
    let local_id = pdf_source_id(bytes);
    resolve_from_extracted(local_id, raw, data_dir, mailto, verify_identity).await
}

/// Upstream enrichment dispatch (first hit wins): arXiv id -> DOI -> CrossRef
/// title search. Every source error is swallowed and the next is tried; `None`
/// means nothing matched and the caller keeps the partial record.
async fn enrich_external(raw: &Extracted, data_dir: &Path, mailto: &str) -> Option<PaperMetadata> {
    if let Some(id) = &raw.arxiv_id {
        if let Ok(m) = arxiv::fetch_by_id(id, data_dir).await {
            return Some(m);
        }
    }
    if let Some(doi) = &raw.doi {
        if let Ok(m) = doi_resolve::resolve_doi(doi, data_dir, mailto).await {
            return Some(m);
        }
    }
    if let Some(title) = &raw.title {
        if let Some(m) = try_crossref_title(title, data_dir, mailto).await {
            return Some(m);
        }
    }
    None
}

/// CrossRef title search: take the first candidate whose title is >= 0.5
/// Jaccard-similar; if it carries a DOI, upgrade via `resolve_doi`, else as-is.
async fn try_crossref_title(title: &str, data_dir: &Path, mailto: &str) -> Option<PaperMetadata> {
    for candidate in crossref::search_by_title(title, 3, mailto).await {
        if !candidate.title.is_empty() && title_similarity(title, &candidate.title) >= 0.5 {
            if let Some(doi) = &candidate.doi {
                if let Ok(m) = doi_resolve::resolve_doi(doi, data_dir, mailto).await {
                    return Some(m);
                }
            }
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::pdf_metadata::identity::sha256;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    // ---- PDF-metadata-first: sufficiency gate + offline identity ----

    #[test]
    fn pdf_metadata_sufficiency_gate() {
        assert!(!pdf_metadata_is_sufficient(&Extracted::default()));

        let full = Extracted {
            title: Some("A Real Title".into()),
            authors: Some("Alice; Bob".into()),
            doi: None,
            arxiv_id: Some("2604.21547v1".into()),
            year: Some(2026),
        };
        assert!(pdf_metadata_is_sufficient(&full));

        // Missing title or authors is insufficient — falls through to
        // enrichment instead of being treated as a resolved record. Same for
        // an authors field of stray separators only: the raw string is
        // non-blank, but split_authors() yields nothing real.
        for degraded in [
            Extracted {
                title: None,
                ..full.clone()
            },
            Extracted {
                authors: None,
                ..full.clone()
            },
            Extracted {
                authors: Some("; ; ".into()),
                ..full.clone()
            },
        ] {
            assert!(!pdf_metadata_is_sufficient(&degraded));
        }

        // Year is NOT required: title + authors alone still short-circuits.
        // With an arXiv id present, `partial_meta_from_raw` derives the date
        // from its embedded YYMM (no network) rather than defaulting to today.
        let no_year = Extracted {
            year: None,
            ..full.clone()
        };
        assert!(pdf_metadata_is_sufficient(&no_year));
        let meta = partial_meta_from_raw("local:x".into(), &no_year);
        assert_eq!(meta.published, NaiveDate::from_ymd_opt(2026, 4, 1).unwrap());

        // No arXiv id either: nothing PDF-derived left, falls back to today.
        let no_year_no_id = Extracted {
            year: None,
            arxiv_id: None,
            ..full.clone()
        };
        let meta = partial_meta_from_raw("local:x".into(), &no_year_no_id);
        assert_eq!(meta.published, Utc::now().date_naive());

        assert_eq!(
            arxiv_id_year_month("2604.21547v1"),
            NaiveDate::from_ymd_opt(2026, 4, 1)
        );
        assert_eq!(
            arxiv_id_year_month("0704.0001"),
            NaiveDate::from_ymd_opt(2007, 4, 1)
        );
        assert_eq!(arxiv_id_year_month("bad"), None);

        let meta = partial_meta_from_raw("local:x".into(), &full);
        assert_eq!(meta.title, "A Real Title");
        assert_eq!(meta.authors, vec!["Alice", "Bob"]);
        assert_eq!(meta.published, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        assert_eq!(meta.source.as_deref(), Some("pdf"));

        // A DOI text-scanned off the page is PDF-derived, so it's kept as a
        // plain field even though it's not trusted for identity (unlike an
        // arXiv id, a bare DOI has no offline/cheap way to confirm what root
        // it maps to — that's exactly what the network DOI resolver does).
        let with_doi = Extracted {
            doi: Some("10.1234/xyz".into()),
            ..full.clone()
        };
        assert_eq!(
            partial_meta_from_raw("local:x".into(), &with_doi)
                .doi
                .as_deref(),
            Some("10.1234/xyz")
        );
    }

    #[test]
    fn split_authors_semicolon_then_comma() {
        // `;` is unambiguous and wins when present.
        assert_eq!(split_authors("Alice; Bob"), vec!["Alice", "Bob"]);
        // No `;`: falls back to `,` — but only when every piece looks like a
        // full name (has whitespace), i.e. a genuine name list.
        assert_eq!(
            split_authors("Alice Smith, Bob Jones"),
            vec!["Alice Smith", "Bob Jones"]
        );
        // A single "Last, First" author (Word/Zotero/EndNote convention) has
        // no `;`, and its two comma-parts are each a single bare token — the
        // repair pass re-merges them, so it stays one author, not two.
        assert_eq!(split_authors("Smith, John"), vec!["Smith, John"]);
        // Several "Last, First" authors chained by commas only (no `;`): each
        // bare-token pair re-merges independently -> two correct authors, not
        // one garbled string and not four fragments.
        assert_eq!(
            split_authors("Smith, John, Doe, Jane"),
            vec!["Smith, John", "Doe, Jane"]
        );
        // Single name, no separator at all.
        assert_eq!(split_authors("Alice Smith"), vec!["Alice Smith"]);
        // A single bare surname (no comma, no whitespace) stays one author —
        // there's no following token to (wrongly) pair it with.
        assert_eq!(split_authors("Smith"), vec!["Smith"]);
        // Stray separators only -> nothing real.
        assert_eq!(split_authors("; ; "), Vec::<String>::new());
        assert_eq!(split_authors(""), Vec::<String>::new());
    }

    #[test]
    fn identity_if_title_matches_gate() {
        let same = crate::test_support::meta("arxiv:1111.11111", 2);
        let same = PaperMetadata {
            title: "Yang-Baxter Integrability and Exceptional-Point Structure".into(),
            ..same
        };
        assert_eq!(
            identity_if_title_matches(
                "Yang-Baxter Integrability and Exceptional-Point Structure in Systems",
                same.clone(),
            ),
            Some(("arxiv:1111.11111".to_string(), 2))
        );

        // A citation match (same id-shaped text, unrelated paper) has an
        // unrelated title — must NOT be adopted as identity.
        let different = PaperMetadata {
            title: "Attention Is All You Need".into(),
            ..same
        };
        assert_eq!(
            identity_if_title_matches(
                "Yang-Baxter Integrability and Exceptional-Point Structure in Systems",
                different,
            ),
            None
        );
    }

    #[test]
    fn identity_candidate_is_at_most_one_arxiv_preferred() {
        // Both present: arXiv wins, DOI is never even considered — regardless
        // of whether an arXiv verification would ultimately succeed. This is
        // what makes the "at most one lookup" guarantee structural rather
        // than a matter of remembering not to fall through after a failure.
        let both = Extracted {
            title: Some("T".into()),
            authors: Some("A".into()),
            doi: Some("10.1234/x".into()),
            arxiv_id: Some("2604.99999".into()),
            year: None,
        };
        assert!(
            matches!(identity_candidate(&both), IdentityCandidate::Arxiv(id) if id == "2604.99999")
        );

        let doi_only = Extracted {
            arxiv_id: None,
            ..both.clone()
        };
        assert!(
            matches!(identity_candidate(&doi_only), IdentityCandidate::Doi(doi) if doi == "10.1234/x")
        );

        let neither = Extracted {
            doi: None,
            arxiv_id: None,
            ..both
        };
        assert!(matches!(
            identity_candidate(&neither),
            IdentityCandidate::None
        ));
    }

    // The short-circuit end to end, through resolve_from_extracted directly
    // (not resolve_pdf_metadata / real pdfium) so it's driven by a synthetic
    // Extracted: title+authors sufficient, no arXiv id and no DOI, so this is
    // provably zero-network (enrich_external, verify_arxiv_identity, and
    // verify_doi_identity are never reachable — there's nothing for any of
    // them to look up). DOI-as-plain-field and DOI-as-identity are exercised
    // separately (`pdf_metadata_sufficiency_gate`'s `with_doi` case, and
    // `identity_if_title_matches_gate`, respectively) since a real DOI in
    // this test would make `verify_doi_identity` hit the network.
    #[tokio::test]
    async fn short_circuit_resolves_from_pdf_metadata_with_no_network_signal() {
        let raw = Extracted {
            title: Some("A Real Title".into()),
            authors: Some("Alice Smith, Bob Jones".into()),
            doi: None,
            arxiv_id: None,
            year: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let (meta, ext) = resolve_from_extracted("local:x".into(), raw, dir.path(), "", true)
            .await
            .unwrap();
        assert_eq!(ext, None);
        assert_eq!(meta.title, "A Real Title");
        assert_eq!(meta.authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(meta.doi, None);
        assert_eq!(meta.published, Utc::now().date_naive());
        assert_eq!(meta.summary, "", "short-circuit never carries an abstract");
        assert_eq!(meta.source_id, "local:x");
    }

    // `verify_identity: false` (the `pdf_import_verify_identity_enabled`
    // setting off) must skip the lookup even when a candidate arXiv id IS
    // present — proving the setting actually gates the network call, not
    // just documents an intent. If this regressed to "true" being ignored
    // or the gate applying only when no candidate exists, this would hang or
    // require network; it does neither.
    #[tokio::test]
    async fn verify_identity_false_skips_lookup_even_with_a_candidate_id() {
        let raw = Extracted {
            title: Some("A Real Title".into()),
            authors: Some("Alice Smith, Bob Jones".into()),
            doi: Some("10.1234/would-be-verified-if-enabled".into()),
            arxiv_id: Some("2604.21547v1".into()),
            year: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let (meta, ext) = resolve_from_extracted("local:x".into(), raw, dir.path(), "", false)
            .await
            .unwrap();
        // No identity attached, despite a real, verifiable arXiv id being present.
        assert_eq!(ext, None);
        // Fields still come straight from the PDF, same as the enabled case.
        assert_eq!(meta.title, "A Real Title");
        assert_eq!(meta.authors, vec!["Alice Smith", "Bob Jones"]);
        // The DOI text is still preserved as a plain field — the setting only
        // gates identity *verification*, not field extraction.
        assert_eq!(
            meta.doi.as_deref(),
            Some("10.1234/would-be-verified-if-enabled")
        );
    }

    // ---- partial-record builder ----

    // Junk bytes extract no arXiv id / DOI / title, so enrich_external short-
    // circuits to None with zero network calls — the offline fallthrough path.
    #[tokio::test]
    async fn resolve_partial_record_shape() {
        let dir = tempfile::tempdir().unwrap();
        let (m, ext) = resolve_pdf_metadata(b"some pdf bytes", dir.path(), "", true)
            .await
            .unwrap();
        assert_eq!(ext, None);
        assert_eq!(m.source.as_deref(), Some("pdf"));
        assert!(m.source_id.starts_with("local:"));
        // deterministic id == local:<sha256[:16]>
        assert_eq!(
            m.source_id,
            format!("local:{}", &hex(&sha256(b"some pdf bytes"))[..16])
        );
    }
}

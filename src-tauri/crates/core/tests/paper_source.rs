//! Integration tests for the `PaperSource` seam — the ones `sources/openalex.rs`,
//! `sources/crossref.rs` and the late `sources/fetch.rs` each promised.
//!
//! No live network. The trait is what makes that possible: a fake adapter stands
//! in for a Provider, and the three real adapters are exercised only along the
//! paths that refuse input *before* any request goes out.

mod common;

use std::sync::Mutex;

use linxiv_core::error::{CoreError, Result};
use linxiv_core::models::PaperMetadata;
use linxiv_core::service::paper as svc_paper;
use linxiv_core::service::source as svc_source;
use linxiv_core::sources::provider::{Arxiv, CrossRef, OpenAlex, PaperSource};
use rusqlite::Connection;
use serde_json::json;

fn meta(source_id: &str, title: &str) -> PaperMetadata {
    serde_json::from_value(json!({
        "source_id": source_id,
        "version": 1,
        "title": title,
        "authors": ["Ada Lovelace"],
        "published": "2024-01-01",
        "summary": "S",
        "source": "fake",
    }))
    .unwrap()
}

/// A Provider that answers from memory and records what it was asked, so the
/// trait can be driven end to end without a request.
struct FakeSource {
    records: Vec<PaperMetadata>,
    /// Last `(query, max_results, sort)` handed to `search`.
    last_search: Mutex<Option<(String, u32, String)>>,
}

impl PaperSource for FakeSource {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn fetch_by_id(&self, paper_id: &str) -> Result<PaperMetadata> {
        self.records
            .iter()
            .find(|m| m.source_id == paper_id)
            .cloned()
            // The contract: a miss is an Err, never a None the caller must unwrap.
            .ok_or_else(|| CoreError::Validation(format!("fake: no record for '{paper_id}'")))
    }

    async fn search(
        &self,
        query: &str,
        max_results: u32,
        sort: &str,
    ) -> Result<Vec<PaperMetadata>> {
        *self.last_search.lock().unwrap() =
            Some((query.to_string(), max_results, sort.to_string()));
        Ok(self
            .records
            .iter()
            .filter(|m| m.title.contains(query))
            .take(max_results as usize)
            .cloned()
            .collect())
    }
}

fn fake() -> FakeSource {
    FakeSource {
        records: vec![
            meta("fake:1", "Attention Is All You Need"),
            meta("fake:2", "Attention Considered Harmful"),
            meta("fake:3", "Something Else"),
        ],
        last_search: Mutex::new(None),
    }
}

/// Generic over the trait — if this compiles and passes for the fake, any
/// adapter can be dropped into the same pipeline.
async fn fetch_and_save<S: PaperSource>(
    src: &S,
    conn: &mut Connection,
    paper_id: &str,
) -> Result<String> {
    let meta = src.fetch_by_id(paper_id).await?;
    let (stored, _) = svc_paper::save_paper_metadata(conn, &meta, None)?;
    Ok(stored)
}

#[tokio::test]
async fn an_adapter_fetch_stores_through_the_service_save_path() {
    let mut conn = common::db();
    let stored = fetch_and_save(&fake(), &mut conn, "fake:1").await.unwrap();
    assert_eq!(stored, "fake:1");

    let got = svc_paper::get(
        &conn,
        &svc_paper::Paper {
            source_id: Some("fake:1".to_string()),
            ..Default::default()
        },
    )
    .unwrap()
    .expect("saved paper is readable");
    assert_eq!(got.title, "Attention Is All You Need");
    assert_eq!(got.source.as_deref(), Some("fake"));
}

#[tokio::test]
async fn a_miss_is_an_err_not_an_absent_value() {
    let mut conn = common::db();
    let err = fetch_and_save(&fake(), &mut conn, "fake:nope")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");
    assert!(err.to_string().contains("fake:nope"));
}

#[tokio::test]
async fn search_hands_the_adapter_every_argument_including_sort() {
    let src = fake();
    let out = src.search("Attention", 5, "newest").await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(
        src.last_search.lock().unwrap().clone(),
        Some(("Attention".to_string(), 5, "newest".to_string()))
    );
    // max_results is a cap, not a hint.
    assert_eq!(
        src.search("Attention", 1, "relevance").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn every_adapter_names_the_provider_its_records_carry() {
    // CONTEXT.md § Provider: PAPER_META.PROVIDER must equal the fetching module.
    assert_eq!(Arxiv::new(std::env::temp_dir()).name(), "arxiv");
    assert_eq!(OpenAlex::new("").name(), "openalex");
    assert_eq!(CrossRef.name(), "crossref");
}

#[tokio::test]
async fn unknown_source_is_refused_before_any_network() {
    let err = svc_source::fetch_by_id("scopus", "123").await.unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");
    assert!(err.to_string().contains("Unknown source 'scopus'"));

    let err = svc_source::search("scopus", "q", 5, "relevance")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");
}

/// Each real adapter, reached through the trait, on a path that refuses before
/// it opens a socket — proof the adapter is wired to its Provider module and
/// that it carries its own configuration rather than taking it per call.
#[tokio::test]
async fn real_adapters_refuse_bad_input_without_leaving_the_process() {
    let arxiv = Arxiv::new(std::env::temp_dir());
    let err = arxiv.fetch_by_id("arxiv:").await.unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");

    let openalex = OpenAlex::new("me@example.org");
    let err = openalex
        .fetch_by_id("openalex:not-a-work")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::OpenAlexInput(_)), "got {err}");

    // CrossRef no longer ignores `sort`: a key it cannot honour is refused
    // rather than silently dropped.
    let err = CrossRef
        .search("attention", 5, "lastUpdated")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");
    assert!(err.to_string().contains("unknown sort"));
}

/// Compiles iff the trait's futures are `Send` — what the service dispatch, and
/// anything that spawns a fetch, needs.
#[allow(dead_code)]
fn futures_are_send() {
    fn assert_send<T: Send>(_: T) {}
    let src = fake();
    assert_send(async move { src.fetch_by_id("fake:1").await });
}

//! Integration tests for the `service::source` dispatch. No live network: the
//! paths exercised here refuse input *before* any request goes out.

use linxiv_core::error::CoreError;
use linxiv_core::service::source as svc_source;
use linxiv_core::sources::{arxiv, crossref, openalex};

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

/// Each Provider module, on a path that refuses before it opens a socket —
/// proof the input validation sits in front of the network call.
#[tokio::test]
async fn provider_modules_refuse_bad_input_without_leaving_the_process() {
    let err = arxiv::fetch_by_id("arxiv:", &std::env::temp_dir())
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");

    let err = openalex::fetch_by_id("openalex:not-a-work", "me@example.org")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::OpenAlexInput(_)), "got {err}");

    // CrossRef does not ignore `sort`: a key it cannot honour is refused
    // rather than silently dropped.
    let err = crossref::search_by_title_checked("attention", 5, "lastUpdated", "me@example.org")
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)), "got {err}");
    assert!(err.to_string().contains("unknown sort"));
}

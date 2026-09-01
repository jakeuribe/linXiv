//! The quarantined CRDT document model. Each struct derives autosurgeon's
//! `Reconcile`/`Hydrate` so it maps to/from an automerge document; `PartialEq`
//! backs the round-trip and merge-convergence assertions.
//!
//! Fields are projected from the canonical `linxiv_core::models` read views
//! (PaperDetails / NoteDetails / ProjectDetails). `color` widens i32→i64 and the
//! note timestamps are stored as ISO strings — automerge has no native i32/date
//! scalar that autosurgeon maps a `NaiveDateTime` onto without a custom impl.

use autosurgeon::{Hydrate, Reconcile};

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedProject {
    pub share_id: String,
    pub name: String,
    pub description: String,
    pub color: Option<i64>,
    pub tags: Vec<String>,
    pub papers: Vec<SharedPaper>,
    pub notes: Vec<SharedNote>,
    pub annotations: Vec<SharedAnnotation>,
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedPaper {
    /// Stable identity within a project; `#[key]` so autosurgeon merges
    /// list edits by paper, not by position.
    #[key]
    pub source_id: String,
    pub version: i64,
    pub published: Option<String>,
    pub title: String,
    pub summary: String,
    pub authors: Vec<String>,
    pub tags: Vec<String>,
    /// Provider metadata needed to reopen non-PDF sources such as lectures.
    #[autosurgeon(missing = "Default::default")]
    pub url: Option<String>,
    #[autosurgeon(missing = "Default::default")]
    pub source: Option<String>,
    /// Blob ticket for the paper's PDF, minted by an e2ee hoster
    /// (`ShareNode::store_pdf_blob`).
    #[autosurgeon(missing = "Default::default")]
    pub pdf_blob: Option<String>,
    /// Index-aligned with `authors`; absent in pre-upgrade docs.
    #[autosurgeon(missing = "Default::default")]
    pub author_orcids: Vec<Option<String>>,
}

impl SharedPaper {
    /// The wire summary `GET /api/share/received/{id}` sends per paper: the
    /// display fields plus `has_pdf` in place of the blob ticket. Named per the
    /// serializer convention — the one home for this projection.
    pub fn to_summary_value(&self) -> serde_json::Value {
        serde_json::json!({
            "source_id": self.source_id,
            "version": self.version,
            "title": self.title,
            "summary": self.summary,
            "authors": self.authors,
            "tags": self.tags,
            "has_pdf": self.pdf_blob.is_some(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedNote {
    /// Stable canonical identity (NOTE.NOTE_UUID); `#[key]` so autosurgeon merges
    /// list edits by note, not by position.
    #[key]
    pub uuid: String,
    /// Paper the note hangs off; absent in pre-upgrade docs.
    #[autosurgeon(missing = "Default::default")]
    pub paper_source_id: Option<String>,
    pub title: String,
    pub body: String,
    /// Provider-neutral playback position for timestamped lecture notes.
    #[autosurgeon(missing = "Default::default")]
    pub media_time_ms: Option<i64>,
    #[autosurgeon(missing = "Default::default")]
    pub media_item_id: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// PDF highlight annotation projected into the snapshot. `anchor` is the opaque
/// highlight-geometry JSON; `comment` is the written comment ("" = highlight-only).
#[derive(Debug, Clone, PartialEq, Reconcile, Hydrate)]
pub struct SharedAnnotation {
    /// Stable canonical identity (ANNOTATION.ANNOTATION_UUID); CRDT list key.
    #[key]
    pub uuid: String,
    pub paper_source_id: String,
    pub anchor: String,
    pub comment: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Lightweight listing view — hydrated counts without the full subgraph.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedSummary {
    pub share_id: String,
    pub name: String,
    pub paper_count: usize,
    pub note_count: usize,
    pub annotation_count: usize,
    pub tag_count: usize,
}

/// Heavyweight listing view — full subgraph, pruned/summarized conservatively.
#[derive(Debug, Clone, PartialEq)]
pub struct FullSummary {
    // NOTE: Not sure this is where this should truly go
    // TODO: Complete here and elsewhere
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the received-share paper summary: display fields + `has_pdf`, never
    /// the blob ticket or ORCIDs.
    #[test]
    fn paper_summary_pins_the_wire_shape() {
        let p = SharedPaper {
            source_id: "arxiv:2401.00001".into(),
            version: 2,
            published: Some("2024-01-01".into()),
            title: "T".into(),
            summary: "S".into(),
            authors: vec!["A".into()],
            tags: vec!["t".into()],
            url: None,
            source: None,
            pdf_blob: Some("ticket".into()),
            author_orcids: vec![None],
        };
        assert_eq!(
            serde_json::to_string(&p.to_summary_value()).unwrap(),
            r#"{"source_id":"arxiv:2401.00001","version":2,"title":"T","summary":"S","authors":["A"],"tags":["t"],"has_pdf":true}"#
        );
    }

    /// Pre-upgrade docs lack `pdf_blob` / `paper_source_id` keys; hydrate must
    /// default them to `None` instead of erroring.
    #[test]
    fn hydrates_doc_missing_optional_keys() {
        // Old-schema shapes: same fields minus the later additions.
        #[derive(Reconcile)]
        struct OldPaper {
            source_id: String,
            version: i64,
            published: Option<String>,
            title: String,
            summary: String,
            authors: Vec<String>,
            tags: Vec<String>,
        }
        #[derive(Reconcile)]
        struct OldNote {
            uuid: String,
            title: String,
            body: String,
            created_at: Option<String>,
            updated_at: Option<String>,
        }

        let mut doc = automerge::AutoCommit::new();
        autosurgeon::reconcile(
            &mut doc,
            OldPaper {
                source_id: "2401.00001".into(),
                version: 1,
                published: None,
                title: "t".into(),
                summary: "s".into(),
                authors: vec![],
                tags: vec![],
            },
        )
        .unwrap();
        let paper: SharedPaper = autosurgeon::hydrate(&doc).unwrap();
        assert_eq!(paper.pdf_blob, None);
        assert_eq!(paper.author_orcids, Vec::<Option<String>>::new());

        let mut doc = automerge::AutoCommit::new();
        autosurgeon::reconcile(
            &mut doc,
            OldNote {
                uuid: "u".into(),
                title: "t".into(),
                body: "b".into(),
                created_at: None,
                updated_at: None,
            },
        )
        .unwrap();
        let note: SharedNote = autosurgeon::hydrate(&doc).unwrap();
        assert_eq!(note.paper_source_id, None);
        assert_eq!(note.media_time_ms, None);
    }
}

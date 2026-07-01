//! Per-entity named queries (plan §5.3). Each `todo!` cites its Python source.
//!
//! NON-NEGOTIABLE notes for whoever fills these:
//!   * Every connection these run on already has `PRAGMA foreign_keys = ON`
//!     (storage::db::open) — never open a raw rusqlite::Connection here.
//!   * papers_fts.paper_id holds the SOURCE_ID *string* (e.g. "arxiv:2204.12985"),
//!     NOT the integer PAPER_ID — the column name is a historical misnomer. The
//!     FTS join is `papers.source_id = papers_fts.paper_id`.
//!   * FTS5 has no UPDATE: refresh an entry with DELETE-then-INSERT, never UPDATE.

pub mod annotation;
pub mod author;
pub mod note;
pub mod paper;
pub mod project;
pub mod search;
pub mod search_history;
pub mod search_state;
pub mod tag;

pub use author::{get_author, get_many as get_authors};
pub use note::get_notes;
pub use paper::{get_paper, list_papers};
pub use project::{get_project, list_projects};
pub use search::search_full_text;
pub use tag::list_tags;

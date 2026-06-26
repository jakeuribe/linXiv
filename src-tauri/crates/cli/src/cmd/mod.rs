//! Per-group command modules. Each defines its `Subcommand` enum + an async
//! `run(cmd, ctx)`. `library` owns search/fetch/list; `misc` owns
//! stats/categories/settings; the rest map 1:1 to a top-level group.

pub mod author;
pub mod bibtex;
pub mod doi;
pub mod library;
pub mod misc;
pub mod note;
pub mod paper;
pub mod pdf;
pub mod project;
pub mod tag;
pub mod trash;

//! Per-group command modules, each a `Subcommand` enum + async `run(cmd, ctx)`.
//! `library` owns search/fetch/list; `misc` owns stats/categories/settings.

pub mod annotation;
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

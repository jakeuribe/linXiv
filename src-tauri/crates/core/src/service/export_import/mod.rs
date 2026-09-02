//! export_import service — Rust port of `service/export_import.py`.
//!
//! A `.lxproj` file is a zip archive: `manifest.json` (project + papers + notes,
//! all keyed by source_id — no local DB ids) plus optional `pdfs/{source_id}_v{n}.pdf`
//! entries. That archive PDF name (`{source_id}_v{version}.pdf`, WITH the `_v`
//! separator) is owned by [`ArchivePdfName`] and is DISTINCT from the on-disk
//! managed name `{safe}v{version}.pdf` (`service::paper::pdf_on_disk_name`) —
//! the separate type/helper keep the two formats unmixable; do NOT unify them.
//!
//! DI seam: every DB-touching fn takes `conn` first; every FS-touching fn takes the
//! resolved `pdf_dir: &Path` as a param. Nothing here reads config.
//!
//! ZIP I/O: `export_project`/`preview_import`/`commit_import` read and write the
//! `.lxproj` archive via the `zip` crate (ZIP_DEFLATED). They wrap the in-memory
//! manifest layer — manifest construction, preview, and the two-phase commit with
//! whole-project rollback — which is independently tested on a decoded `Manifest`
//! + PDF bytes.
//!
//! Rollback parity: a mid-import failure SOFT-DELETES the project (Python
//! `_project.delete`, i.e. trash) and surfaces `CoreError::ProjectImport`. Papers
//! saved before the failure stay in the library exactly as in Python — the
//! "rollback" is scoped to the project, undone by trashing it.
//!
//! Layout: `dto` (manifest wire model), `export` (manifest build + zip write),
//! `archive` (zip read boundary), `import` (two-phase commit + rollback),
//! `share_id` (path-safety validator). The public API is flat
//! (`service::export_import::X`) via the re-exports below.

mod archive;
mod dto;
mod export;
mod import;
mod share_id;

pub use archive::preview_import;
pub use dto::{ImportPreview, OnConflict};
pub use export::export_project;
pub use import::commit_import;
pub use share_id::valid_share_id;

#[cfg(test)]
mod tests;

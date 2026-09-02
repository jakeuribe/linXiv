//! Import side: preview over a decoded manifest and the two-phase commit with
//! whole-project rollback.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use rusqlite::Connection;

use super::archive::{open_archive, read_manifest};
use super::dto::{ArchivePdf, ArchivePdfName, ImportPreview, Manifest, OnConflict};
use crate::error::{CoreError, Result};
use crate::models::{validate_anchor, AnnotationIn, NoteIn, ProjectIn};
use crate::service::{annotation, note, paper, project};

/// `preview_import` over an already-decoded manifest (no DB, no zip).
pub fn preview_from_manifest(manifest: &Manifest) -> ImportPreview {
    // Archives predating the summary counts store 0; fall back to the array length.
    ImportPreview {
        project_name: manifest.project.name.clone(),
        description: manifest.project.description.clone(),
        paper_count: if manifest.summary.paper_count > 0 {
            manifest.summary.paper_count
        } else {
            manifest.papers.len()
        },
        note_count: if manifest.summary.note_count > 0 {
            manifest.summary.note_count
        } else {
            manifest.notes.len()
        },
        // The annotations array is the ground truth at preview time.
        annotation_count: manifest.annotations.len(),
        has_pdfs: manifest.summary.has_pdfs,
        format_version: manifest.format_version,
    }
}

/// Two-phase commit over an already-decoded manifest + decoded PDF bytes. Creates a
/// fresh project, imports papers (merge/overwrite), links them, writes bundled PDFs,
/// then imports notes. On ANY failure the project is soft-deleted (trash) and a
/// `CoreError::ProjectImport` is returned — papers saved before the failure remain
/// (Python parity). Returns the new project_fk.
pub fn commit_from_manifest(
    conn: &mut Connection,
    manifest: &Manifest,
    pdfs: &[ArchivePdf],
    on_conflict: OnConflict,
    pdf_dir: &Path,
) -> Result<i64> {
    let color = match &manifest.project.color_hex {
        Some(hex) => Some(project::color_from_hex(hex)?),
        None => None,
    };

    let project_fk = project::create(
        conn,
        &ProjectIn {
            name: manifest.project.name.clone(),
            description: manifest.project.description.clone(),
            color,
            tags: manifest.project.tags.clone(),
            source_fks: Vec::new(),
        },
    )?;
    match commit_body(conn, project_fk, manifest, pdfs, on_conflict, pdf_dir) {
        Ok(()) => {
            // Restore the archived share identity after a successful import.
            if let Some(share_id) = &manifest.project.share_id {
                if let Ok(u) = uuid::Uuid::parse_str(share_id) {
                    if let Err(e) = project::adopt_share_id(conn, project_fk, &u.to_string()) {
                        tracing::warn!("share_id adoption failed for project {project_fk}: {e}");
                    }
                }
            }
            Ok(project_fk)
        }
        Err(e) => {
            // Trash the partially-built project (Python `_project.delete`).
            tracing::warn!("import failed, trashing project {project_fk}: {e}");
            let _ = project::delete(
                conn,
                &project::Project {
                    project_fk: Some(project_fk),
                },
            );
            Err(CoreError::ProjectImport(e.to_string()))
        }
    }
}

fn commit_body(
    conn: &mut Connection,
    project_fk: i64,
    manifest: &Manifest,
    pdfs: &[ArchivePdf],
    on_conflict: OnConflict,
    pdf_dir: &Path,
) -> Result<()> {
    // Resolve every archived paper to a SOURCE_FK, in first-seen order. The
    // id→fk map feeds the note/annotation passes so they never re-resolve roots.
    let mut source_ids: Vec<String> = Vec::new();
    let mut resolved: HashMap<String, i64> = HashMap::new();
    for pe in &manifest.papers {
        let source_id = pe.source_id.clone();
        let source_fk = match paper::get_paper_root(conn, &source_id)? {
            Some(root) => {
                if root.status == "deleted" {
                    paper::restore(conn, &paper::PaperRef::source(source_id.clone()))?;
                }
                if on_conflict == OnConflict::Overwrite {
                    // Unvalidated: an archive replays already-stored metadata, so it
                    // is not held to the Paper Repair input rules the front doors apply.
                    paper::repair_paper_unvalidated(conn, root.source_fk, &pe.to_metadata())?;
                }
                // UNION the archive paper's tags onto the existing paper (Python
                // `_paper.add_paper_tags`) so a merge-import never discards them.
                if !pe.tags.is_empty() {
                    paper::add_paper_tags(conn, &source_id, &pe.tags)?;
                }
                root.source_fk
            }
            None => {
                let meta = pe.to_metadata();
                let extra = (!pe.tags.is_empty()).then(|| pe.tags.clone());
                paper::save_paper_metadata(conn, &meta, extra.as_deref())?;
                paper::ensure_paper_root(conn, &source_id)?
            }
        };
        resolved.insert(source_id.clone(), source_fk);
        source_ids.push(source_id);
    }

    // Link every imported paper to the new project. An unresolved id here means the
    // saved id and the membership lookup disagree on id form (e.g. surrounding
    // whitespace) — fail so the project rolls back rather than linking partially.
    if !source_ids.is_empty() {
        let failed = project::add_papers(conn, project_fk, &source_ids)?;
        if !failed.is_empty() {
            let shown: Vec<_> = failed.iter().take(5).cloned().collect();
            return Err(CoreError::ProjectImport(format!(
                "{} imported paper(s) could not be linked to the project: {:?}",
                failed.len(),
                shown
            )));
        }
    }

    import_pdfs(conn, pdfs, &source_ids, pdf_dir)?;
    import_notes(conn, project_fk, manifest, &resolved)?;
    import_annotations(conn, project_fk, manifest, &resolved)?;
    Ok(())
}

/// Write bundled PDFs into `pdf_dir` under their ARCHIVE basename
/// (`{source_id}_v{version}.pdf` — kept verbatim, NOT the on-disk `v` form) and
/// record the path on the matching paper version. A PDF naming a version that
/// wasn't imported is skipped and its extracted file removed (Python parity).
pub(super) fn import_pdfs(
    conn: &mut Connection,
    pdfs: &[ArchivePdf],
    source_ids: &[String],
    pdf_dir: &Path,
) -> Result<()> {
    if pdfs.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(pdf_dir).map_err(|e| CoreError::Internal(e.to_string()))?;

    let known: HashSet<&str> = source_ids.iter().map(String::as_str).collect();
    for entry in pdfs {
        let Some(name) = ArchivePdfName::parse_entry(&entry.archive_name) else {
            continue;
        };
        if !known.contains(name.source_id.as_str()) {
            continue;
        }

        // Dest keeps the archive basename VERBATIM (not re-encoded): a
        // non-canonical stem like `a_v01.pdf` must land on disk unchanged.
        let basename = entry
            .archive_name
            .rsplit('/')
            .next()
            .unwrap_or(&entry.archive_name);
        let dest = pdf_dir.join(basename);
        std::fs::write(&dest, &entry.bytes).map_err(|e| CoreError::Internal(e.to_string()))?;
        let dest_str = dest.to_string_lossy().to_string();

        if let Err(e) = paper::mark_pdf_saved(conn, &name.source_id, &dest_str, name.version) {
            // Bundled PDF names a version that wasn't imported: drop the file, log, skip.
            let _ = std::fs::remove_file(&dest);
            tracing::warn!("import: skipping PDF {basename}: {e}");
        }
    }
    Ok(())
}

fn import_notes(
    conn: &Connection,
    project_fk: i64,
    manifest: &Manifest,
    resolved: &HashMap<String, i64>,
) -> Result<()> {
    for nd in &manifest.notes {
        let Some(paper_source_id) = &nd.paper_source_id else {
            continue;
        };
        let Some(&source_fk) = resolved.get(paper_source_id) else {
            continue;
        };

        // Re-pin to a specific PAPER version if the note named one.
        let paper_id = match nd.paper_version {
            Some(v) if v != 0 => paper::get(
                conn,
                &paper::PaperRef::Source {
                    source_id: paper_source_id.clone(),
                    version: Some(v),
                },
            )?
            .map(|p| p.paper_id),
            _ => None,
        };

        note::create(
            conn,
            &NoteIn {
                source_fk,
                title: nd.title.clone(),
                content: nd.content.clone(),
                paper_id,
                project_fk: Some(project_fk),
                uuid: nd.uuid.clone(),
            },
        )?;
    }
    Ok(())
}

fn import_annotations(
    conn: &Connection,
    project_fk: i64,
    manifest: &Manifest,
    resolved: &HashMap<String, i64>,
) -> Result<()> {
    for ad in &manifest.annotations {
        // Same anchor rule as the live write boundaries, but skip-not-fail: one
        // bad archived annotation must not abort the whole import.
        if let Err(msg) = validate_anchor(&ad.anchor) {
            tracing::warn!(
                "import: skipping annotation for '{}': {msg}",
                ad.paper_source_id
            );
            continue;
        }
        let Some(&source_fk) = resolved.get(&ad.paper_source_id) else {
            continue;
        };
        annotation::create(
            conn,
            &AnnotationIn {
                source_fk,
                anchor: ad.anchor.clone(),
                comment: ad.comment.clone(),
                project_fk: Some(project_fk),
                uuid: ad.uuid.clone(),
            },
        )?;
    }
    Ok(())
}

/// Parse a `.lxproj` archive (manifest + every `pdfs/*.pdf` entry) and import it.
/// Returns the new project_fk.
pub fn commit_import(
    conn: &mut Connection,
    zip_path: &Path,
    on_conflict: OnConflict,
    pdf_dir: &Path,
) -> Result<i64> {
    let mut archive = open_archive(zip_path)?;
    let manifest = read_manifest(&mut archive, &zip_path.display().to_string())?;

    // Collect every bundled PDF entry (basename parsing happens in import_pdfs).
    let mut pdfs = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        let name = entry.name().to_string();
        if name.starts_with("pdfs/") && name.ends_with(".pdf") {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| CoreError::Internal(e.to_string()))?;
            pdfs.push(ArchivePdf {
                archive_name: name,
                bytes,
            });
        }
    }

    commit_from_manifest(conn, &manifest, &pdfs, on_conflict, pdf_dir)
}

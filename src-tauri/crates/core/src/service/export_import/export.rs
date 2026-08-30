//! Export side: manifest construction plus the `.lxproj` zip write.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::Connection;

use super::dto::{
    AnnotationEntry, ArchivePdfName, Manifest, NoteEntry, PaperEntry, ProjectEntry, Summary,
    FORMAT_VERSION,
};
use crate::error::{CoreError, Result};
use crate::models::PaperDetails;
use crate::service::{annotation, note, paper, project};

/// Build the in-memory manifest for a project plus the list of
/// `(archive_name, local_pdf_path)` entries to bundle. This is all of
/// `export_project` except the final zip packaging. `pdf_dir` resolves any
/// relative `PDF_PATH` stored on a paper row.
pub fn build_manifest(
    conn: &Connection,
    project_fk: i64,
    include_pdfs: bool,
    pdf_dir: &Path,
) -> Result<(Manifest, Vec<(String, PathBuf)>)> {
    let details = project::get_required(conn, project_fk)?;

    let papers = paper::get_many(
        conn,
        &paper::Papers {
            source_fks: Some(details.source_fks.clone()),
            ..Default::default()
        },
    )?;

    let paper_entries: Vec<PaperEntry> = papers
        .iter()
        .map(|p| PaperEntry::from_details(conn, p))
        .collect::<Result<Vec<_>>>()?;
    let note_entries = collect_note_entries(conn, project_fk)?;
    let annotation_entries = collect_annotation_entries(conn, project_fk)?;
    let pdf_files = if include_pdfs {
        collect_pdf_files(&papers, pdf_dir)
    } else {
        Vec::new()
    };
    // Python: `color_to_hex(details.color) if details.color else None` — 0 is falsy.
    let color_hex = details.color.filter(|&c| c != 0).map(project::color_to_hex);

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        exported_at: Some(Utc::now().to_rfc3339()),
        summary: Summary {
            paper_count: paper_entries.len(),
            note_count: note_entries.len(),
            annotation_count: annotation_entries.len(),
            has_pdfs: !pdf_files.is_empty(),
        },
        project: ProjectEntry {
            name: details.name,
            description: details.description,
            color_hex,
            tags: details.project_tags,
            share_id: details.share_id,
        },
        papers: paper_entries,
        notes: note_entries,
        annotations: annotation_entries,
    };
    Ok((manifest, pdf_files))
}

/// Archive note records for a project's notes, keyed by source_id.
fn collect_note_entries(conn: &Connection, project_fk: i64) -> Result<Vec<NoteEntry>> {
    let notes = note::get_many(
        conn,
        &note::Notes {
            project_fk: Some(project_fk),
            ..Default::default()
        },
    )?;
    let mut note_entries = Vec::new();
    for n in &notes {
        let Some(source_id) = paper::get_source_id(conn, n.source_fk)? else {
            continue; // Python skips notes whose source_id no longer resolves.
        };
        let version = match n.paper_id_fk {
            Some(pid) => paper::get(conn, &paper::PaperRef::Id(pid))?.map(|p| p.version),
            None => None,
        };
        note_entries.push(NoteEntry {
            paper_source_id: Some(source_id),
            paper_version: version,
            title: n.title.clone(),
            content: n.content.clone(),
            uuid: Some(n.uuid.clone()),
        });
    }
    Ok(note_entries)
}

/// Archive annotation records for a project's annotations, keyed by source_id.
fn collect_annotation_entries(conn: &Connection, project_fk: i64) -> Result<Vec<AnnotationEntry>> {
    let annotations = annotation::get_many(
        conn,
        &annotation::Annotations {
            project_fk: Some(project_fk),
            ..Default::default()
        },
    )?;
    let mut annotation_entries = Vec::new();
    for a in &annotations {
        let Some(source_id) = paper::get_source_id(conn, a.source_fk)? else {
            continue; // skip annotations whose source_id no longer resolves.
        };
        annotation_entries.push(AnnotationEntry {
            paper_source_id: source_id,
            anchor: a.anchor.clone(),
            comment: a.comment.clone(),
            uuid: Some(a.uuid.clone()),
        });
    }
    Ok(annotation_entries)
}

/// archive_name -> local path; only files that actually exist on disk.
fn collect_pdf_files(papers: &[PaperDetails], pdf_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut pdf_files: Vec<(String, PathBuf)> = Vec::new();
    for p in papers {
        let Some(stored) = &p.pdf_path else { continue };
        let local = pdf_dir.join(stored);
        if local.is_file() {
            let name = ArchivePdfName {
                source_id: p.source_id.clone(),
                version: p.version,
            };
            pdf_files.push((name.to_string(), local));
        }
    }
    pdf_files
}

/// Write a project to a `.lxproj` ZIP_DEFLATED archive at `dest_path` (`.lxproj`
/// is forced as the extension): `manifest.json` (pretty JSON) plus each bundled
/// PDF under its archive name `pdfs/{source_id}_v{version}.pdf`.
pub fn export_project(
    conn: &Connection,
    project_fk: i64,
    dest_path: &Path,
    include_pdfs: bool,
    pdf_dir: &Path,
) -> Result<PathBuf> {
    let (manifest, pdf_files) = build_manifest(conn, project_fk, include_pdfs, pdf_dir)?;
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| CoreError::Internal(e.to_string()))?;
    let dest = dest_path.with_extension("lxproj");

    let file = std::fs::File::create(&dest).map_err(|e| CoreError::Internal(e.to_string()))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let zerr = |e: zip::result::ZipError| CoreError::Internal(e.to_string());
    let werr = |e: std::io::Error| CoreError::Internal(e.to_string());

    zw.start_file("manifest.json", opts).map_err(zerr)?;
    zw.write_all(manifest_json.as_bytes()).map_err(werr)?;

    for (archive_name, local_path) in &pdf_files {
        let bytes = std::fs::read(local_path).map_err(werr)?;
        zw.start_file(archive_name.as_str(), opts).map_err(zerr)?;
        zw.write_all(&bytes).map_err(werr)?;
    }
    zw.finish().map_err(zerr)?;
    Ok(dest)
}

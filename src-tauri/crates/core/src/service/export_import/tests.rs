use super::import::import_pdfs;
use super::*;
use crate::error::CoreError;
use crate::models::{AnnotationIn, NoteIn, PaperMetadata, ProjectIn, Status};
use crate::service::{annotation, note, paper, project};
use crate::test_support::db;
use chrono::{NaiveDate, Utc};
use rusqlite::Connection;

// ── ArchivePdfName (pins the exact legacy import-loop behavior) ─────────

#[test]
fn archive_pdf_name_display() {
    let n = ArchivePdfName {
        source_id: "2204.12985".into(),
        version: 1,
    };
    assert_eq!(n.to_string(), "pdfs/2204.12985_v1.pdf");
}

#[test]
fn archive_pdf_name_parse_skips_and_accepts_like_import_loop() {
    // Skipped: non-.pdf suffix, stem without "_v".
    assert_eq!(ArchivePdfName::parse_entry("pdfs/a_v1.txt"), None);
    assert_eq!(ArchivePdfName::parse_entry("pdfs/av1.pdf"), None);
    // Accepted regardless of directory prefix — only the basename is
    // parsed (the `pdfs/` filter lives at the zip layer).
    for path in [
        "pdfs/a_v2.pdf",
        "other/a_v2.pdf",
        "a_v2.pdf",
        "x/y/a_v2.pdf",
    ] {
        assert_eq!(
            ArchivePdfName::parse_entry(path),
            Some(ArchivePdfName {
                source_id: "a".into(),
                version: 2,
            }),
            "{path}"
        );
    }
    // Non-numeric or empty version falls back to 1.
    assert_eq!(
        ArchivePdfName::parse_entry("pdfs/a_vX.pdf")
            .unwrap()
            .version,
        1
    );
    assert_eq!(
        ArchivePdfName::parse_entry("pdfs/a_v.pdf").unwrap().version,
        1
    );
}

#[test]
fn archive_pdf_name_round_trips() {
    // Property-style: parse(display(n)) == n. Includes source_ids that
    // themselves contain "_v" — rfind splits on the LAST "_v", which is
    // always the encoded `_v{version}` suffix.
    for sid in [
        "2204.12985",
        "arxiv:1",
        "foo_v2_bar",
        "a_v",
        "_v",
        "x_v5",
        "",
    ] {
        for version in [0, 1, 7, 12, 130] {
            let n = ArchivePdfName {
                source_id: sid.into(),
                version,
            };
            assert_eq!(ArchivePdfName::parse_entry(&n.to_string()), Some(n));
        }
    }
}

fn meta(source_id: &str, version: i64, title: &str, tags: &[&str]) -> PaperMetadata {
    PaperMetadata {
        source_id: source_id.into(),
        version,
        title: title.into(),
        authors: vec!["Alice".into()],
        published: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        updated: None,
        summary: "sum".into(),
        category: Some("cs.LG".into()),
        categories: Some(vec!["cs.LG".into()]),
        doi: None,
        journal_ref: None,
        comment: None,
        url: Some("http://x".into()),
        tags: (!tags.is_empty()).then(|| tags.iter().map(|t| t.to_string()).collect()),
        source: Some("arxiv".into()),
        author_orcids: None,
    }
}

fn paper_entry(source_id: &str, version: i64, title: &str, tags: &[&str]) -> PaperEntry {
    PaperEntry {
        source_id: source_id.into(),
        version,
        title: title.into(),
        authors: vec!["Bob".into()],
        author_orcids: vec![],
        published: NaiveDate::from_ymd_opt(2023, 5, 5),
        updated: None,
        summary: "s".into(),
        category: Some("cs.AI".into()),
        categories: vec!["cs.AI".into()],
        doi: None,
        journal_ref: None,
        comment: None,
        url: None,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        source: Some("arxiv".into()),
    }
}

fn base_manifest(name: &str, papers: Vec<PaperEntry>, notes: Vec<NoteEntry>) -> Manifest {
    Manifest {
        format_version: 1,
        exported_at: None,
        summary: Summary::default(),
        project: ProjectEntry {
            name: name.into(),
            description: "desc".into(),
            color_hex: Some("#00ff00".into()),
            tags: vec!["imported".into()],
            share_id: None,
        },
        papers,
        notes,
        annotations: Vec::new(),
    }
}

const ANCHOR: &str = r##"{"v":1,"version":1,"page":1,"color":"#ffd400","quote":"q","rects":[{"x":0,"y":0,"w":0.5,"h":0.1}]}"##;

#[test]
fn build_and_commit_round_trip_annotations() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();

    // Seed a paper + project + a project-scoped annotation on it.
    paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "P", &[]), None).unwrap();
    let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
    let pid = project::create(
        &mut conn,
        &ProjectIn {
            name: "Proj".into(),
            description: "d".into(),
            color: None,
            tags: vec![],
            source_fks: vec![fk1],
        },
    )
    .unwrap();
    annotation::create(
        &conn,
        &AnnotationIn {
            source_fk: fk1,
            anchor: ANCHOR.into(),
            comment: "hi".into(),
            project_fk: Some(pid),
            uuid: None,
        },
    )
    .unwrap();

    let (m, _) = build_manifest(&conn, pid, false, tmp.path()).unwrap();
    assert_eq!(m.annotations.len(), 1);
    assert_eq!(m.annotations[0].paper_source_id, "arxiv:1");
    assert_eq!(m.annotations[0].comment, "hi");
    let exported_uuid = m.annotations[0].uuid.clone().expect("annotation uuid");
    assert_eq!(exported_uuid.len(), 36);

    // Commit into a fresh DB and confirm the annotation lands project-scoped.
    let mut conn2 = db();
    let new_pid = commit_from_manifest(&mut conn2, &m, &[], OnConflict::Merge, tmp.path()).unwrap();
    let anns = annotation::get_many(
        &conn2,
        &annotation::Annotations {
            project_fk: Some(new_pid),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(anns.len(), 1);
    assert_eq!(anns[0].anchor, ANCHOR);
    assert_eq!(anns[0].comment, "hi");
    assert_eq!(anns[0].uuid, exported_uuid);
}

#[test]
fn build_manifest_round_trips_project_papers_notes_and_pdfs() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();
    let pdf_dir = tmp.path();

    // Two papers; one has a PDF on disk (relative path resolved against pdf_dir).
    paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Paper One", &["ml"]), None).unwrap();
    paper::save_paper_metadata(&mut conn, &meta("arxiv:2", 1, "Paper Two", &[]), None).unwrap();
    let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
    let fk2 = paper::ensure_paper_root(&mut conn, "arxiv:2").unwrap();
    std::fs::write(pdf_dir.join("arxiv_1v1.pdf"), b"PDFBYTES").unwrap();
    paper::set_pdf_path(&conn, "arxiv:1", "arxiv_1v1.pdf", Some(1)).unwrap();

    let pid = project::create(
        &mut conn,
        &ProjectIn {
            name: "My Proj".into(),
            description: "d".into(),
            color: Some(0x00ff00),
            tags: vec!["t".into()],
            source_fks: vec![fk1, fk2],
        },
    )
    .unwrap();

    // A note pinned to arxiv:1 v1.
    let pid_v1 = paper::get(
        &conn,
        &paper::PaperRef::Source {
            source_id: "arxiv:1".into(),
            version: Some(1),
        },
    )
    .unwrap()
    .unwrap()
    .paper_id;
    note::create(
        &conn,
        &NoteIn {
            source_fk: fk1,
            title: "n".into(),
            content: "body".into(),
            media_time_ms: None,
            media_item_id: None,
            paper_id: Some(pid_v1),
            project_fk: Some(pid),
            uuid: None,
        },
    )
    .unwrap();

    let (m, pdf_files) = build_manifest(&conn, pid, true, pdf_dir).unwrap();
    assert_eq!(m.project.name, "My Proj");
    assert_eq!(m.project.color_hex.as_deref(), Some("#00ff00"));
    assert_eq!(m.papers.len(), 2);
    assert_eq!(m.summary.paper_count, 2);
    assert!(m.summary.has_pdfs);
    assert_eq!(m.notes.len(), 1);
    assert_eq!(m.notes[0].paper_source_id.as_deref(), Some("arxiv:1"));
    assert_eq!(m.notes[0].paper_version, Some(1));
    // Archive PDF name uses the `_v` separator, not the on-disk `v` form.
    assert_eq!(pdf_files.len(), 1);
    assert_eq!(pdf_files[0].0, "pdfs/arxiv:1_v1.pdf");
    let _ = fk2;
}

#[test]
fn build_manifest_missing_project_is_typed_not_found() {
    let conn = db();
    let tmp = tempfile::tempdir().unwrap();
    assert!(matches!(
        build_manifest(&conn, 999, false, tmp.path()).unwrap_err(),
        CoreError::ProjectNotFound(999)
    ));
}

#[test]
fn preview_from_manifest_uses_summary_then_falls_back_to_lengths() {
    let mut m = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
    // summary zeroed -> fall back to vec lengths
    let p = preview_from_manifest(&m);
    assert_eq!(p.paper_count, 1);
    assert_eq!(p.note_count, 0);
    assert_eq!(p.project_name, "P");
    // explicit summary wins
    m.summary = Summary {
        paper_count: 7,
        note_count: 3,
        annotation_count: 0,
        has_pdfs: true,
    };
    let p = preview_from_manifest(&m);
    assert_eq!(p.paper_count, 7);
    assert_eq!(p.note_count, 3);
    assert!(p.has_pdfs);
}

#[test]
fn commit_creates_project_links_papers_notes_and_writes_pdf() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();
    let pdf_dir = tmp.path();

    let manifest = base_manifest(
        "Imported",
        vec![paper_entry("arxiv:1", 1, "New Paper", &["ml"])],
        vec![NoteEntry {
            paper_source_id: Some("arxiv:1".into()),
            paper_version: Some(1),
            title: "note".into(),
            content: "c".into(),
            media_time_ms: None,
            media_item_id: None,
            uuid: None,
        }],
    );
    let pdfs = vec![ArchivePdf {
        archive_name: "pdfs/arxiv:1_v1.pdf".into(),
        bytes: b"BYTES".to_vec(),
    }];

    let pid =
        commit_from_manifest(&mut conn, &manifest, &pdfs, OnConflict::Merge, pdf_dir).unwrap();

    // Project created with tags + colour from the manifest.
    let got = project::get(
        &conn,
        &project::Project {
            project_fk: Some(pid),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(got.name, "Imported");
    assert_eq!(got.project_tags, vec!["imported"]);
    assert_eq!(got.color, Some(0x00ff00));
    assert_eq!(got.source_fks.len(), 1, "paper linked to the new project");

    // Paper metadata saved.
    let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
        .unwrap()
        .unwrap();
    assert_eq!(p.title, "New Paper");

    // PDF written under the archive basename + recorded on the paper.
    let on_disk = pdf_dir.join("arxiv:1_v1.pdf");
    assert!(on_disk.is_file(), "archive-named pdf written to disk");
    assert_eq!(
        p.pdf_path.as_deref(),
        Some(on_disk.to_string_lossy().as_ref())
    );
    assert!(p.has_pdf);

    // Note imported and pinned.
    let notes = note::get_many(
        &conn,
        &note::Notes {
            project_fk: Some(pid),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "note");
    assert!(notes[0].paper_id_fk.is_some());
}

#[test]
fn commit_merge_keeps_existing_metadata_overwrite_replaces_it() {
    let tmp = tempfile::tempdir().unwrap();
    let manifest = base_manifest(
        "P",
        vec![paper_entry("arxiv:1", 1, "Archive Title", &[])],
        vec![],
    );

    // MERGE: pre-existing paper keeps its stored title, still gets linked.
    {
        let mut conn = db();
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Stored Title", &[]), None)
            .unwrap();
        let pid =
            commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
        let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
            .unwrap()
            .unwrap();
        assert_eq!(p.title, "Stored Title", "merge does not overwrite metadata");
        assert_eq!(
            project::get(
                &conn,
                &project::Project {
                    project_fk: Some(pid)
                }
            )
            .unwrap()
            .unwrap()
            .source_fks
            .len(),
            1
        );
    }
    // OVERWRITE: stored paper is repaired to the archive's title.
    {
        let mut conn = db();
        paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Stored Title", &[]), None)
            .unwrap();
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Overwrite, tmp.path()).unwrap();
        let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
            .unwrap()
            .unwrap();
        assert_eq!(
            p.title, "Archive Title",
            "overwrite repairs metadata from the archive"
        );
    }
}

#[test]
fn commit_restores_soft_deleted_paper_on_merge() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();
    paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "T", &[]), None).unwrap();
    paper::delete(&mut conn, &paper::PaperRef::source("arxiv:1".into())).unwrap();
    assert!(paper::is_paper_deleted(&conn, "arxiv:1").unwrap());

    let manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
    commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
    assert!(
        !paper::is_paper_deleted(&conn, "arxiv:1").unwrap(),
        "import restored the trashed paper"
    );
}

#[test]
fn commit_rolls_back_project_when_a_paper_cannot_be_linked() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();
    // Whitespace in the source_id: it's saved verbatim, but add_papers trims it,
    // so the membership lookup misses -> link fails -> whole project rolled back.
    let manifest = base_manifest(
        "Doomed",
        vec![paper_entry("  spacey:1  ", 1, "T", &[])],
        vec![],
    );

    let err =
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap_err();
    assert!(matches!(err, CoreError::ProjectImport(_)));

    // The project must NOT survive as active — it was trashed (Python `_project.delete`).
    let active = project::get_many(
        &conn,
        &project::Projects {
            status: Some(Status::Active),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        active.iter().all(|p| p.name != "Doomed"),
        "failed import leaves no active project"
    );
    // It IS present in trash.
    let trashed = project::list_deleted(&conn).unwrap();
    assert!(
        trashed.iter().any(|p| p.name == "Doomed"),
        "rolled-back project is in trash"
    );
}

#[test]
fn import_pdfs_skips_unknown_version_and_removes_file() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();
    let pdf_dir = tmp.path();
    paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "T", &[]), None).unwrap();

    // v9 was never imported for this paper -> mark_pdf_saved errors, file removed.
    let pdfs = vec![ArchivePdf {
        archive_name: "pdfs/arxiv:1_v9.pdf".into(),
        bytes: b"X".to_vec(),
    }];
    import_pdfs(&mut conn, &pdfs, &["arxiv:1".into()], pdf_dir).unwrap();
    assert!(
        !pdf_dir.join("arxiv:1_v9.pdf").exists(),
        "orphan pdf removed after failed mark"
    );

    // A pdf whose source_id wasn't imported is ignored entirely.
    let pdfs = vec![ArchivePdf {
        archive_name: "pdfs/other:2_v1.pdf".into(),
        bytes: b"X".to_vec(),
    }];
    import_pdfs(&mut conn, &pdfs, &["arxiv:1".into()], pdf_dir).unwrap();
    assert!(!pdf_dir.join("other:2_v1.pdf").exists());
}

#[test]
fn paper_entry_to_metadata_defaults_published_today() {
    let mut pe = paper_entry("arxiv:1", 1, "T", &[]);
    pe.published = None;
    let m = pe.to_metadata();
    assert_eq!(m.published, Utc::now().date_naive());
}

#[test]
fn zip_export_then_import_round_trips_to_a_fresh_db() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();
    let export_pdf_dir = tmp.path().join("export_pdfs");
    std::fs::create_dir_all(&export_pdf_dir).unwrap();

    // Seed: one paper with a real PDF on disk + a note pinned to it.
    paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "Paper One", &["ml"]), None).unwrap();
    let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
    let pdf_path = export_pdf_dir.join("arxiv_1v1.pdf");
    std::fs::write(&pdf_path, b"PDFCONTENT").unwrap();
    paper::set_pdf_path(&conn, "arxiv:1", pdf_path.to_str().unwrap(), Some(1)).unwrap();
    paper::set_has_pdf(&conn, "arxiv:1", 1, true).unwrap();

    let pid = project::create(
        &mut conn,
        &ProjectIn {
            name: "Round Trip".into(),
            description: "d".into(),
            color: Some(0x00ff00),
            tags: vec!["t".into()],
            source_fks: vec![fk1],
        },
    )
    .unwrap();
    let pv1 = paper::get(
        &conn,
        &paper::PaperRef::Source {
            source_id: "arxiv:1".into(),
            version: Some(1),
        },
    )
    .unwrap()
    .unwrap()
    .paper_id;
    note::create(
        &conn,
        &NoteIn {
            source_fk: fk1,
            title: "n".into(),
            content: "body".into(),
            media_time_ms: None,
            media_item_id: None,
            paper_id: Some(pv1),
            project_fk: Some(pid),
            uuid: None,
        },
    )
    .unwrap();

    // Export -> .lxproj is forced as the extension.
    let dest = tmp.path().join("out");
    let written = export_project(&conn, pid, &dest, true, &export_pdf_dir).unwrap();
    assert_eq!(written.extension().and_then(|e| e.to_str()), Some("lxproj"));
    assert!(written.is_file());

    // Preview without touching the DB.
    let preview = preview_import(&written).unwrap();
    assert_eq!(preview.project_name, "Round Trip");
    assert_eq!(preview.paper_count, 1);
    assert_eq!(preview.note_count, 1);
    assert!(preview.has_pdfs);

    // Commit into a FRESH db + fresh pdf dir.
    let mut conn2 = db();
    let import_pdf_dir = tmp.path().join("import_pdfs");
    let new_pid = commit_import(&mut conn2, &written, OnConflict::Merge, &import_pdf_dir).unwrap();

    let proj = project::get(
        &conn2,
        &project::Project {
            project_fk: Some(new_pid),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(proj.name, "Round Trip");
    assert_eq!(proj.source_fks.len(), 1);

    let p = paper::get(&conn2, &paper::PaperRef::source("arxiv:1".into()))
        .unwrap()
        .unwrap();
    assert_eq!(p.title, "Paper One");
    assert!(p.has_pdf);

    // PDF written under the ARCHIVE basename (`_v`), bytes intact, path recorded.
    let imported_pdf = import_pdf_dir.join("arxiv:1_v1.pdf");
    assert!(imported_pdf.is_file());
    assert_eq!(std::fs::read(&imported_pdf).unwrap(), b"PDFCONTENT");
    assert_eq!(
        p.pdf_path.as_deref(),
        Some(imported_pdf.to_string_lossy().as_ref())
    );

    let notes = note::get_many(
        &conn2,
        &note::Notes {
            project_fk: Some(new_pid),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "n");
    assert!(notes[0].paper_id_fk.is_some());
    let orig = note::get_many(
        &conn,
        &note::Notes {
            project_fk: Some(pid),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(notes[0].uuid, orig[0].uuid);
    assert_eq!(notes[0].uuid.len(), 36);
}

#[test]
fn build_and_commit_round_trip_author_orcids() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();

    let mut m = meta("arxiv:1", 1, "P", &[]);
    m.authors = vec!["Alice".into(), "Bob".into()];
    m.author_orcids = Some(vec![Some("0000-0001".into()), None]);
    paper::save_paper_metadata(&mut conn, &m, None).unwrap();
    let fk1 = paper::ensure_paper_root(&mut conn, "arxiv:1").unwrap();
    let pid = project::create(
        &mut conn,
        &ProjectIn {
            name: "Proj".into(),
            description: "d".into(),
            color: None,
            tags: vec![],
            source_fks: vec![fk1],
        },
    )
    .unwrap();

    let (manifest, _) = build_manifest(&conn, pid, false, tmp.path()).unwrap();
    assert_eq!(manifest.papers.len(), 1);
    assert_eq!(
        manifest.papers[0].author_orcids,
        vec![Some("0000-0001".to_string()), None]
    );

    // Commit into a fresh DB and confirm AUTHOR_ORCID lands (fill-if-null).
    let mut conn2 = db();
    commit_from_manifest(&mut conn2, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
    fn orcid(conn: &Connection, name: &str) -> Option<String> {
        conn.query_row(
            "SELECT AUTHOR_ORCID FROM AUTHOR WHERE AUTHOR_FULL_NAME = ?",
            [name],
            |r| r.get(0),
        )
        .unwrap()
    }
    assert_eq!(orcid(&conn2, "Alice").as_deref(), Some("0000-0001"));
    assert_eq!(orcid(&conn2, "Bob"), None);
}

#[test]
fn commit_merge_unions_tags_onto_existing_paper() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();

    // Pre-existing paper carries tag A.
    paper::save_paper_metadata(&mut conn, &meta("arxiv:1", 1, "T", &["A"]), None).unwrap();

    // Merge-import the same paper carrying tag B.
    let manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &["B"])], vec![]);
    commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();

    let p = paper::get(&conn, &paper::PaperRef::source("arxiv:1".into()))
        .unwrap()
        .unwrap();
    let mut tags = p.tags.clone();
    tags.sort();
    assert_eq!(
        tags,
        vec!["A".to_string(), "B".to_string()],
        "merge unions tags, not discard"
    );

    // Relational half re-synced too.
    let relational: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM PAPER_TO_TAG WHERE SOURCE_ID = ?",
            ["arxiv:1"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(relational, 2);
}

/// SECURITY BOUNDARY. `route/share.rs` joins share ids onto `share_dir` at six
/// call sites, so anything this accepts becomes a path component. The
/// commit_adopts/rejects pair below only exercises it through the importer;
/// this pins the validator itself.
#[test]
fn valid_share_id_rejects_everything_that_could_escape_share_dir() {
    // A real share id — a uuid v4 — is the only shape that must pass.
    assert!(valid_share_id("3f2b8c1a-9d4e-4f6a-8b2c-1d5e7f9a0b3c"));
    assert!(valid_share_id("plain-id_123"));

    for bad in [
        "",                      // empty — joins to the share_dir itself
        "..",                    // parent
        "../etc/passwd",         // classic traversal
        "..\\windows\\system32", // traversal, Windows separator
        "a/../../b",             // traversal mid-string
        "a..b",                  // any embedded `..`, conservatively
        ".",                     // current dir
        ".hidden",               // any leading dot
        "sub/dir",               // separator
        "sub\\dir",              // separator, Windows
        "/etc/passwd",           // absolute, Unix
        "C:evil",                // Windows drive-relative — join() escapes
        "C:\\Windows",           // absolute, Windows
        "\\\\server\\share",     // UNC
        "..\u{202e}gnp.exe",     // RTL override still carries `..`
        "\u{ff0e}\u{ff0e}/etc",  // fullwidth dots: not `..`, but `/` catches it
    ] {
        assert!(!valid_share_id(bad), "must reject {bad:?}");
    }

    // Documented gap, not an escape: unicode lookalikes with no separator and no
    // ASCII `..` are accepted as opaque names. They stay inside share_dir.
    assert!(valid_share_id("\u{ff0e}\u{ff0e}"));
}

#[test]
fn commit_adopts_valid_share_id() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();

    let share_id = "3f2b8c1a-9d4e-4f6a-8b2c-1d5e7f9a0b3c";
    let mut manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
    manifest.project.share_id = Some(share_id.into());

    let pid =
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();

    let proj = project::get(
        &conn,
        &project::Project {
            project_fk: Some(pid),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        proj.share_id,
        Some(share_id.into()),
        "share_id adopted from manifest"
    );
}

#[test]
fn commit_rejects_invalid_share_id() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();

    // Traversal, absolute, and dotfile share_ids must NOT be adopted.
    let invalid_ids = vec!["../evil", "/etc/passwd", ".hidden"];
    for invalid_id in invalid_ids {
        let mut manifest = base_manifest("P", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
        manifest.project.share_id = Some(invalid_id.into());

        let pid =
            commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
        let proj = project::get(
            &conn,
            &project::Project {
                project_fk: Some(pid),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            proj.share_id, None,
            "invalid share_id '{}' rejected",
            invalid_id
        );
    }
}

#[test]
fn commit_rejects_duplicate_share_id() {
    let mut conn = db();
    let tmp = tempfile::tempdir().unwrap();

    let share_id = "7a1c4e2d-6b3f-4a8e-9c0d-2e5f7a9b1c4d";
    let mut manifest = base_manifest("P1", vec![paper_entry("arxiv:1", 1, "T", &[])], vec![]);
    manifest.project.share_id = Some(share_id.into());

    // First import claims the share_id.
    let pid1 =
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
    let proj1 = project::get(
        &conn,
        &project::Project {
            project_fk: Some(pid1),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(proj1.share_id, Some(share_id.into()));

    // Second import with the same manifest (same share_id): adopt fails on the
    // live claimant, the import still succeeds, and the project has no share_id.
    manifest.project.name = "P2".into();
    let pid2 =
        commit_from_manifest(&mut conn, &manifest, &[], OnConflict::Merge, tmp.path()).unwrap();
    let proj2 = project::get(
        &conn,
        &project::Project {
            project_fk: Some(pid2),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        proj2.share_id, None,
        "second project does not claim the duplicate share_id"
    );
}

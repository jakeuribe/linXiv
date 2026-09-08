//! editor_project service.
//!
//! An *editor project* is a NOTE whose `content` carries a small frontmatter block
//! declaring it owns an on-disk LaTeX vault:
//!
//! ```text
//! ---
//! linxiv-editor-vault: true
//! projectName: My Draft
//! mainFile: main.tex
//! ---
//! <optional body>
//! ```
//!
//! Note access goes through the sibling note service; vault FS access through
//! `service::vault` (its `safe_path`/`write_file`/`list_files` carry the
//! trust-boundary guard). Standalone projects attach to the sentinel root
//! `texbrain:local`. Vault roots map note_id -> `vault_dir/note_<id>`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use crate::models::{NoteDetails, NoteIn};
use crate::service::note::{self, Note};
use crate::service::vault;
use crate::storage::queries::{note as note_q, paper};

/// Sentinel paper root for standalone editor projects (not about a specific paper).
pub const STANDALONE_SOURCE_ID: &str = "texbrain:local";

/// Frontmatter key flagging a note as an editor project owning a vault.
pub const VAULT_FLAG: &str = "linxiv-editor-vault";

/// Starter main file written into a fresh vault.
pub const DEFAULT_TEX: &str = r#"\documentclass{article}

\title{Untitled Document}
\author{Author}
\date{\today}

\begin{document}
\maketitle

\section{Introduction}
Hello, world!

\end{document}
"#;

// ── frontmatter parse / build ───────────────────────────────────────────────────

/// Parse a note body's frontmatter map. A note with no leading `---` block (or an
/// unterminated one) yields an empty map; the body is never materialized.
pub fn parse_frontmatter(content: &str) -> HashMap<String, String> {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return HashMap::new();
    }
    let mut meta = HashMap::new();
    for line in lines {
        if line.trim() == "---" {
            return meta;
        }
        if let Some((k, v)) = line.split_once(':') {
            meta.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    // Unterminated fence: not frontmatter at all.
    HashMap::new()
}

/// Collapse CR/LF to spaces and trim. Frontmatter is line-oriented, so a newline
/// in a value would terminate/forge the block — never let one through.
fn sanitize_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ").trim().to_string()
}

/// Serialize the frontmatter fence + optional body. Sanitizes again here (defense
/// in depth) so a stray newline can never break or inject into the fence.
pub fn build_content(project_name: &str, main_file: &str, body: &str) -> String {
    format!(
        "---\n{VAULT_FLAG}: true\nprojectName: {}\nmainFile: {}\n---\n{body}",
        sanitize_line(project_name),
        sanitize_line(main_file),
    )
}

fn is_editor_project(meta: &HashMap<String, String>) -> bool {
    meta.get(VAULT_FLAG)
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// One editor project's listing summary (camelCase wire keys).
#[derive(Debug, Clone, Serialize, PartialEq, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub struct EditorProjectSummary {
    pub note_id: i64,
    pub project_name: String,
    pub main_file: String,
    pub source_fk: i64,
    pub project_id: Option<i64>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
pub struct EditorProjectsResponse {
    pub projects: Vec<EditorProjectSummary>,
}

fn to_summary(note: &NoteDetails, meta: &HashMap<String, String>) -> EditorProjectSummary {
    // Fallback chain treats "" as unset.
    let project_name = meta
        .get("projectName")
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| Some(note.title.clone()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| format!("project {}", note.note_id));
    let main_file = meta
        .get("mainFile")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "main.tex".to_string());
    EditorProjectSummary {
        note_id: note.note_id,
        project_name,
        main_file,
        source_fk: note.source_fk,
        project_id: note.project_id,
        updated_at: note
            .updated_at
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S").to_string()),
    }
}

/// `{mainFile, files, projectName}` the host pushes to the editor. `files` is empty:
/// the guest mounts the vault and pulls every file lazily over the FS RPC.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocOpenPayload {
    pub main_file: String,
    pub files: HashMap<String, String>,
    pub project_name: String,
}

/// Absolute directory backing one editor project's vault root.
fn vault_root(vault_dir: &Path, note_id: i64) -> PathBuf {
    vault_dir.join(format!("note_{note_id}"))
}

// ── operations used by the /api/editor routes ────────────────────────────────────

/// Editor-project notes (frontmatter-flagged), newest first, optionally scoped
/// to a linXiv project.
pub fn list_projects(
    conn: &rusqlite::Connection,
    project_id: Option<i64>,
) -> Result<Vec<EditorProjectSummary>> {
    let mut out: Vec<EditorProjectSummary> = Vec::new();
    // SQL prefilters on the flag substring + project scope so we never load every
    // note body; parse_frontmatter stays the exactness guard (the substring could
    // appear in a plain note's body).
    for note in note_q::list_notes_containing(conn, VAULT_FLAG, project_id)? {
        let meta = parse_frontmatter(&note.content);
        if !is_editor_project(&meta) {
            continue;
        }
        out.push(to_summary(&note, &meta));
    }
    // Newest first; None updatedAt sorts last.
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(out)
}

/// Result of `create_project` — the 3-key wire shape, not a full summary.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedProject {
    pub note_id: i64,
    pub project_name: String,
    pub main_file: String,
}

/// Create an editor-project note + scaffold its vault with a starter main file.
/// `main_file` defaults to "main.tex" at the binary layer.
pub fn create_project(
    conn: &mut rusqlite::Connection,
    vault_dir: &Path,
    project_name: &str,
    main_file: &str,
    source_id: Option<&str>,
    project_id: Option<i64>,
) -> Result<CreatedProject> {
    let name = Some(sanitize_line(project_name))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Untitled".to_string());
    let main = Some(sanitize_line(main_file))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "main.tex".to_string());
    // Validate the main file is a safe, contained relative path BEFORE creating the
    // note, so a bad name fails clean rather than orphaning a note with no vault.
    // (Validated against note 0's root; the relpath shape is what matters.)
    vault::safe_path(&vault_root(vault_dir, 0), &main)?;

    // Only "" (not whitespace) falls back to the standalone sentinel.
    let source_id = source_id
        .filter(|s| !s.is_empty())
        .unwrap_or(STANDALONE_SOURCE_ID);
    let source_fk = paper::ensure_paper_root(conn, source_id.trim())?;

    let note_id = note::create(
        conn,
        &NoteIn {
            source_fk,
            paper_id: None,
            project_fk: project_id,
            title: name.clone(),
            content: build_content(&name, &main, ""),
            uuid: None,
        },
    )?
    .note_id;

    // Scaffold the vault; on failure roll back the note so no flagged project is left
    // without a vault.
    if let Err(e) = vault::write_file(&vault_root(vault_dir, note_id), &main, DEFAULT_TEX, false) {
        let _ = note::delete(
            conn,
            &Note {
                note_id: Some(note_id),
            },
        );
        return Err(e);
    }

    Ok(CreatedProject {
        note_id,
        project_name: name,
        main_file: main,
    })
}

/// `(note, frontmatter)` for an editor-project note, or `None` if the note is
/// missing or is not an editor project.
pub fn get_meta(
    conn: &rusqlite::Connection,
    note_id: i64,
) -> Result<Option<(NoteDetails, HashMap<String, String>)>> {
    let note = match note::get(
        conn,
        &Note {
            note_id: Some(note_id),
        },
    )? {
        Some(n) => n,
        None => return Ok(None),
    };
    let meta = parse_frontmatter(&note.content);
    if !is_editor_project(&meta) {
        return Ok(None);
    }
    Ok(Some((note, meta)))
}

/// `true` iff the note exists and is an editor project. The per-FS-RPC vault
/// ownership guard: reads only the content column instead of hydrating the full
/// note row like `get_meta`.
pub fn is_editor_project_note(conn: &rusqlite::Connection, note_id: i64) -> Result<bool> {
    Ok(note_q::get_note_content(conn, note_id)?
        .is_some_and(|c| is_editor_project(&parse_frontmatter(&c))))
}

/// Assemble the DocOpenPayload for the editor, or `None` if not an editor project.
/// The recorded main file may be stale; fall back to a present `.tex` so the
/// project never opens empty.
pub fn get_doc(
    conn: &rusqlite::Connection,
    vault_dir: &Path,
    note_id: i64,
) -> Result<Option<DocOpenPayload>> {
    let (note, meta) = match get_meta(conn, note_id)? {
        Some(x) => x,
        None => return Ok(None),
    };
    let summary = to_summary(&note, &meta);
    let mut main = summary.main_file;

    let existing = vault::list_files(&vault_root(vault_dir, note_id))?;
    if !existing.contains(&main) {
        if existing.iter().any(|p| p == "main.tex") {
            main = "main.tex".to_string();
        } else if let Some(first) = existing
            .iter()
            .filter(|p| p.to_lowercase().ends_with(".tex"))
            .min()
        {
            main = first.clone();
        }
    }

    Ok(Some(DocOpenPayload {
        main_file: main,
        files: HashMap::new(),
        project_name: summary.project_name,
    }))
}

/// Delete a note and, if it is an editor project, its vault tree. The frontmatter
/// is read first — after the row is gone the vault can no longer be identified.
/// `false` if no note matched (nothing deleted).
pub fn delete_note(conn: &rusqlite::Connection, vault_dir: &Path, note_id: i64) -> Result<bool> {
    let is_editor_project = is_editor_project_note(conn, note_id)?;
    if !note::delete(
        conn,
        &Note {
            note_id: Some(note_id),
        },
    )? {
        return Ok(false);
    }
    if is_editor_project {
        vault::delete_vault(&vault_root(vault_dir, note_id));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;
    use crate::test_support::db;

    #[test]
    fn parse_frontmatter_variants() {
        // Full block: meta parsed, body after the closing fence ignored.
        let m = parse_frontmatter(
            "---\nlinxiv-editor-vault: true\nprojectName: My Draft\n---\nhello\nworld",
        );
        assert_eq!(m.get(VAULT_FLAG).map(String::as_str), Some("true"));
        assert_eq!(m.get("projectName").map(String::as_str), Some("My Draft"));

        // No leading fence.
        assert!(parse_frontmatter("just a note").is_empty());

        // Unterminated fence: no partial parse.
        assert!(parse_frontmatter("---\nprojectName: X\nno closing fence").is_empty());

        // Empty.
        assert!(parse_frontmatter("").is_empty());
    }

    #[test]
    fn build_content_sanitizes_and_roundtrips() {
        // A newline in the project name must not forge/terminate the fence.
        let c = build_content("evil\n---\nmainFile: hacked.tex", "main.tex", "body");
        let m = parse_frontmatter(&c);
        assert!(is_editor_project(&m));
        assert_eq!(
            m.get("projectName").map(String::as_str),
            Some("evil --- mainFile: hacked.tex"),
        );
        assert_eq!(m.get("mainFile").map(String::as_str), Some("main.tex"));
        assert!(c.ends_with("\n---\nbody"));
    }

    /// Seed two editor-project notes (one project-scoped) + one plain note.
    fn seed_notes(conn: &rusqlite::Connection) {
        conn.execute(
            "INSERT INTO PAPER_ROOTS (SOURCE_FK, SOURCE_ID) VALUES (1, 'arxiv:1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO PROJECT (PROJECT_FK, NAME) VALUES (5, 'P')",
            [],
        )
        .unwrap();
        let ed = build_content("Draft A", "main.tex", "");
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (1, NULL, 'Draft A', ?1, '2024-01-01 10:00:00', '2024-01-01 10:00:00')",
            [&ed],
        )
        .unwrap();
        let ed2 = build_content("Draft B", "main.tex", "");
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (1, 5, 'Draft B', ?1, '2024-02-02 10:00:00', '2024-02-02 10:00:00')",
            [&ed2],
        )
        .unwrap();
        // Plain note, not an editor project — must be skipped.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (1, NULL, 'plain', 'just an annotation', '2024-03-03 10:00:00', '2024-03-03 10:00:00')",
            [],
        )
        .unwrap();
        // Mentions the flag in its body but has no frontmatter — the SQL prefilter
        // matches it, so this pins that parse_frontmatter still excludes it.
        conn.execute(
            "INSERT INTO NOTE (SOURCE_FK, PROJECT_FK, TITLE, NOTE, CREATED_AT, UPDATED_AT) \
             VALUES (1, NULL, 'impostor', 'talking about linxiv-editor-vault: true in prose', \
             '2024-04-04 10:00:00', '2024-04-04 10:00:00')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn list_projects_filters_and_sorts() {
        let conn = db();
        conn.execute("INSERT INTO PROJECT (PROJECT_FK, NAME) VALUES (5, 'P')", [])
            .unwrap();
        seed_notes(&conn);

        // All editor projects, newest first (Draft B updated after Draft A).
        let all = list_projects(&conn, None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].project_name, "Draft B");
        assert_eq!(all[1].project_name, "Draft A");
        assert_eq!(all[0].updated_at.as_deref(), Some("2024-02-02T10:00:00"));
        assert_eq!(all[0].main_file, "main.tex");

        // Scoped to project 5: only Draft B.
        let scoped = list_projects(&conn, Some(5)).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].project_name, "Draft B");
    }

    /// The vault step is the reason every consumer deletes notes through here:
    /// dropping the row alone leaves `note_<id>/` on disk forever.
    #[test]
    fn delete_note_removes_the_vault_tree() {
        let mut conn = db();
        let vault = tempfile::tempdir().unwrap();
        let created =
            create_project(&mut conn, vault.path(), "Draft", "main.tex", None, None).unwrap();
        let root = vault.path().join(format!("note_{}", created.note_id));
        assert!(root.is_dir());

        assert!(delete_note(&conn, vault.path(), created.note_id).unwrap());
        assert!(!root.exists());
        assert!(get_meta(&conn, created.note_id).unwrap().is_none());
        // Second delete finds no row.
        assert!(!delete_note(&conn, vault.path(), created.note_id).unwrap());
    }

    /// A plain note owns no vault, so the delete must not touch the vault dir.
    #[test]
    fn delete_note_leaves_non_editor_vault_alone() {
        let conn = db();
        let vault = tempfile::tempdir().unwrap();
        seed_notes(&conn);
        let plain_id = note::list_all(&conn)
            .unwrap()
            .into_iter()
            .find(|n| n.title == "plain")
            .unwrap()
            .note_id;
        let stray = vault.path().join(format!("note_{plain_id}"));
        std::fs::create_dir_all(&stray).unwrap();

        assert!(delete_note(&conn, vault.path(), plain_id).unwrap());
        assert!(stray.is_dir());
    }

    #[test]
    fn create_scaffolds_vault_and_get_doc_reads_it() {
        let mut conn = db();
        let vault = tempfile::tempdir().unwrap();

        let created = create_project(
            &mut conn,
            vault.path(),
            "My Paper",
            "main.tex",
            None, // standalone -> texbrain:local root created
            None,
        )
        .unwrap();
        assert_eq!(created.project_name, "My Paper");
        assert_eq!(created.main_file, "main.tex");

        // The note is a real editor project, attached to the sentinel root.
        let (note, meta) = get_meta(&conn, created.note_id).unwrap().unwrap();
        assert!(is_editor_project(&meta));
        let sid = paper::get_source_id(&conn, note.source_fk)
            .unwrap()
            .unwrap();
        assert_eq!(sid, STANDALONE_SOURCE_ID);

        // The starter file is really on disk under the note's vault root.
        let f = vault
            .path()
            .join(format!("note_{}", created.note_id))
            .join("main.tex");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), DEFAULT_TEX);

        // get_doc returns the recorded main file + an empty files map.
        let doc = get_doc(&conn, vault.path(), created.note_id)
            .unwrap()
            .unwrap();
        assert_eq!(doc.main_file, "main.tex");
        assert!(doc.files.is_empty());
        assert_eq!(doc.project_name, "My Paper");
    }

    #[test]
    fn create_rejects_unsafe_main_before_touching_db() {
        let mut conn = db();
        let vault = tempfile::tempdir().unwrap();
        let err =
            create_project(&mut conn, vault.path(), "X", "../escape.tex", None, None).unwrap_err();
        // vault::safe_path maps traversal to BadRequest (HTTP 400).
        assert!(matches!(err, CoreError::BadRequest(_)));
        // No note was created (validation happens before any insert).
        assert!(note::list_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn create_rolls_back_note_when_scaffold_fails() {
        let mut conn = db();
        // vault_dir is a regular FILE, so create_dir_all(note_root) fails -> rollback.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("not_a_dir");
        std::fs::write(&bogus, b"x").unwrap();

        let err = create_project(&mut conn, &bogus, "X", "main.tex", None, None).unwrap_err();
        assert!(matches!(err, CoreError::Internal(_)));
        // The note was deleted on the scaffold failure.
        assert!(note::list_all(&conn).unwrap().is_empty());
    }

    #[test]
    fn get_doc_falls_back_to_present_tex() {
        let mut conn = db();
        let vault = tempfile::tempdir().unwrap();
        let created = create_project(&mut conn, vault.path(), "P", "main.tex", None, None).unwrap();

        // Simulate an in-editor rename: remove main.tex, drop a different .tex.
        let root = vault.path().join(format!("note_{}", created.note_id));
        std::fs::remove_file(root.join("main.tex")).unwrap();
        std::fs::write(root.join("zebra.tex"), b"\\documentclass{article}").unwrap();

        // Recorded main (main.tex) is gone -> falls back to the present .tex.
        let doc = get_doc(&conn, vault.path(), created.note_id)
            .unwrap()
            .unwrap();
        assert_eq!(doc.main_file, "zebra.tex");
    }

    #[test]
    fn get_meta_none_for_missing_and_plain_notes() {
        let conn = db();
        seed_notes(&conn);
        assert!(get_meta(&conn, 9999).unwrap().is_none()); // missing
                                                           // The plain note is not an editor project.
        let plain = note::list_all(&conn)
            .unwrap()
            .into_iter()
            .find(|n| n.title == "plain")
            .unwrap();
        assert!(get_meta(&conn, plain.note_id).unwrap().is_none());
        assert!(get_doc(&conn, Path::new("/nope"), plain.note_id)
            .unwrap()
            .is_none());
    }
}

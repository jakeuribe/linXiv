"""Tests for migrate_db.py — full migration pipeline from old (blue) to new (green) schema."""
from __future__ import annotations

import json
import sqlite3
import sys
import os
from pathlib import Path

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# ---------------------------------------------------------------------------
# Old-schema DDL — built from what migrate_data.sql actually references.
# Does NOT include a VERSION table (old schema predates versioning).
# ---------------------------------------------------------------------------

_OLD_SCHEMA_SQL = """
CREATE TABLE papers (
    paper_id          TEXT    NOT NULL,
    version           INTEGER NOT NULL DEFAULT 1,
    title             TEXT    NOT NULL DEFAULT '',
    url               TEXT,
    published         TEXT,
    updated           TEXT,
    category          TEXT,
    categories        TEXT,
    doi               TEXT,
    journal_ref       TEXT,
    comment           TEXT,
    summary           TEXT,
    authors           TEXT,
    tags              TEXT,
    has_pdf           INTEGER NOT NULL DEFAULT 0,
    source            TEXT,
    pdf_path          TEXT,
    full_text         TEXT,
    downloaded_source INTEGER DEFAULT 0,
    PRIMARY KEY (paper_id, version)
);

CREATE TABLE projects (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL,
    description  TEXT    DEFAULT '',
    color        INTEGER,
    created_at   TEXT,
    updated_at   TEXT,
    archived_at  TEXT,
    project_tags TEXT,
    status       TEXT    DEFAULT 'active',
    paper_ids    TEXT
);

CREATE TABLE notes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    paper_id   TEXT,
    project_id INTEGER,
    title      TEXT    DEFAULT '',
    content    TEXT    DEFAULT '',
    created_at TEXT,
    updated_at TEXT
);
"""

# Path to the real v0.1.0 DB shipped with the test suite
_REAL_OLD_DB = Path(__file__).parent / "test_file" / "papers_v_0_1_0.db"


def _build_old_db(path: str) -> None:
    """Populate a minimal old-schema DB with representative fixture data."""
    conn = sqlite3.connect(path)
    conn.executescript(_OLD_SCHEMA_SQL)

    # Two versions of one arxiv paper (multi-author, tagged, has full_text)
    conn.execute(
        "INSERT INTO papers (paper_id, version, title, authors, tags, source, category, full_text) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (
            "2401.00001", 1, "Test Paper v1",
            json.dumps(["Alice Smith", "Bob Jones"]),
            json.dumps(["ml", "transformers"]),
            "arxiv", "cs.LG",
            "This paper studies transformers.",
        ),
    )
    conn.execute(
        "INSERT INTO papers (paper_id, version, title, authors, tags, source, category) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        (
            "2401.00001", 2, "Test Paper v2",
            json.dumps(["Alice Smith", "Bob Jones"]),
            json.dumps(["ml", "transformers"]),
            "arxiv", "cs.LG",
        ),
    )

    # An openalex paper with a single-token author name and no tags
    conn.execute(
        "INSERT INTO papers (paper_id, version, title, authors, tags, source, category) "
        "VALUES (?, ?, ?, ?, ?, ?, ?)",
        (
            "W3123456789", 1, "OpenAlex Paper",
            json.dumps(["Madonna"]),
            json.dumps([]),
            "openalex", "math.CO",
        ),
    )

    # A paper with NULL source (gets 'linxiv:' prefix)
    conn.execute(
        "INSERT INTO papers (paper_id, version, title, authors, tags, source) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (
            "manual001", 1, "Manually Added Paper",
            json.dumps(["Some Author"]),
            json.dumps([]),
            None,
        ),
    )

    # One project referencing the arxiv and openalex papers, with a project tag
    conn.execute(
        "INSERT INTO projects (id, name, description, project_tags, paper_ids, status) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (
            1, "My Project", "A test project",
            json.dumps(["ml"]),
            json.dumps(["2401.00001", "W3123456789"]),
            "active",
        ),
    )

    # One valid note (has a paper_id — will be migrated)
    conn.execute(
        "INSERT INTO notes (paper_id, project_id, title, content) VALUES (?, ?, ?, ?)",
        ("2401.00001", 1, "My Note", "Some content"),
    )

    # One orphan note (NULL paper_id — cannot be migrated, logged as WARNING)
    conn.execute(
        "INSERT INTO notes (paper_id, project_id, title, content) VALUES (?, ?, ?, ?)",
        (None, 1, "Orphan Note", "Cannot be migrated"),
    )

    conn.commit()
    conn.close()


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture()
def old_db(tmp_path):
    path = str(tmp_path / "old.db")
    _build_old_db(path)
    return path


@pytest.fixture()
def new_db(tmp_path):
    return str(tmp_path / "new.db")


@pytest.fixture()
def migrated_db(old_db, new_db):
    """Run the full migration and return a connection to the new DB."""
    from migrate_db import run_migration
    run_migration(old_db, new_db, force=True)
    conn = sqlite3.connect(new_db)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    yield conn
    conn.close()


# ---------------------------------------------------------------------------
# Row-count verification
# ---------------------------------------------------------------------------

class TestMigrationRowCounts:
    def test_paper_count(self, migrated_db):
        count = migrated_db.execute("SELECT COUNT(*) FROM PAPER").fetchone()[0]
        assert count == 4  # v1+v2 of arxiv paper, v1 of openalex, v1 of null-source

    def test_paper_roots_count(self, migrated_db):
        count = migrated_db.execute("SELECT COUNT(*) FROM PAPER_ROOTS").fetchone()[0]
        assert count == 3  # arxiv:2401.00001, openalex:W3123456789, linxiv:manual001

    def test_paper_meta_count_matches_paper(self, migrated_db):
        papers = migrated_db.execute("SELECT COUNT(*) FROM PAPER").fetchone()[0]
        metas  = migrated_db.execute("SELECT COUNT(*) FROM PAPER_META").fetchone()[0]
        assert papers == metas

    def test_project_count(self, migrated_db):
        count = migrated_db.execute("SELECT COUNT(*) FROM PROJECT").fetchone()[0]
        assert count == 1

    def test_valid_notes_migrated(self, migrated_db):
        # Only the note with a paper_id survives; orphan is skipped
        count = migrated_db.execute("SELECT COUNT(*) FROM NOTE").fetchone()[0]
        assert count == 1

    def test_project_to_paper_count(self, migrated_db):
        count = migrated_db.execute("SELECT COUNT(*) FROM PROJECT_TO_PAPER").fetchone()[0]
        assert count == 2  # arxiv:2401.00001 and openalex:W3123456789 in project

    def test_tags_deduplicated(self, migrated_db):
        # "ml" appears in paper tags AND project tags — should be one TAG row
        count = migrated_db.execute("SELECT COUNT(*) FROM TAG WHERE TAG = 'ml'").fetchone()[0]
        assert count == 1

    def test_project_to_tag_count(self, migrated_db):
        count = migrated_db.execute("SELECT COUNT(*) FROM PROJECT_TO_TAG").fetchone()[0]
        assert count == 1  # one project tag ("ml")

    def test_paper_to_author_count(self, migrated_db):
        # arxiv v1: Alice+Bob, arxiv v2: Alice+Bob, openalex v1: Madonna, null-source v1: Some Author — 6
        count = migrated_db.execute("SELECT COUNT(*) FROM PAPER_TO_AUTHOR").fetchone()[0]
        assert count == 6

    def test_fts_populated_for_full_text_papers(self, migrated_db):
        count = migrated_db.execute("SELECT COUNT(*) FROM papers_fts").fetchone()[0]
        assert count == 1  # only paper1 v1 had full_text


# ---------------------------------------------------------------------------
# Soft-delete columns (STATUS / DELETED_AT on PAPER_ROOTS)
# ---------------------------------------------------------------------------

class TestSoftDeleteMigration:
    def test_all_roots_are_active(self, migrated_db):
        """Every migrated PAPER_ROOTS row must have STATUS = 'active'."""
        non_active = migrated_db.execute(
            "SELECT COUNT(*) FROM PAPER_ROOTS WHERE STATUS != 'active'"
        ).fetchone()[0]
        assert non_active == 0

    def test_no_roots_have_deleted_at(self, migrated_db):
        """DELETED_AT must be NULL for all migrated rows — old schema had no soft-delete."""
        with_deleted_at = migrated_db.execute(
            "SELECT COUNT(*) FROM PAPER_ROOTS WHERE DELETED_AT IS NOT NULL"
        ).fetchone()[0]
        assert with_deleted_at == 0

    def test_papers_view_shows_all_migrated_papers(self, migrated_db):
        """The 'papers' view filters by STATUS='active', so it must return all migrated rows."""
        view_count = migrated_db.execute("SELECT COUNT(*) FROM papers").fetchone()[0]
        table_count = migrated_db.execute("SELECT COUNT(*) FROM PAPER").fetchone()[0]
        assert view_count == table_count

    def test_deleted_papers_view_is_empty(self, migrated_db):
        """Nothing should appear in deleted_papers after a fresh migration."""
        count = migrated_db.execute("SELECT COUNT(*) FROM deleted_papers").fetchone()[0]
        assert count == 0


# ---------------------------------------------------------------------------
# FK integrity
# ---------------------------------------------------------------------------

class TestFKIntegrity:
    def test_no_fk_violations(self, migrated_db):
        violations = migrated_db.execute("PRAGMA foreign_key_check").fetchall()
        assert list(violations) == [], f"FK violations: {[dict(r) for r in violations]}"

    def test_paper_source_fk_valid(self, migrated_db):
        orphans = migrated_db.execute(
            "SELECT COUNT(*) FROM PAPER p "
            "LEFT JOIN PAPER_ROOTS r ON r.SOURCE_FK = p.SOURCE_FK "
            "WHERE r.SOURCE_FK IS NULL"
        ).fetchone()[0]
        assert orphans == 0

    def test_paper_meta_paper_fk_valid(self, migrated_db):
        orphans = migrated_db.execute(
            "SELECT COUNT(*) FROM PAPER_META m "
            "LEFT JOIN PAPER p ON p.PAPER_ID = m.PAPER_ID "
            "WHERE p.PAPER_ID IS NULL"
        ).fetchone()[0]
        assert orphans == 0

    def test_note_source_fk_valid(self, migrated_db):
        orphans = migrated_db.execute(
            "SELECT COUNT(*) FROM NOTE n "
            "LEFT JOIN PAPER_ROOTS r ON r.SOURCE_FK = n.SOURCE_FK "
            "WHERE r.SOURCE_FK IS NULL"
        ).fetchone()[0]
        assert orphans == 0


# ---------------------------------------------------------------------------
# Author name splitting
# ---------------------------------------------------------------------------

class TestAuthorNameSplitting:
    def test_two_token_name(self, migrated_db):
        row = migrated_db.execute(
            "SELECT AUTHOR_FIRST, AUTHOR_LAST FROM AUTHOR WHERE AUTHOR_FULL_NAME = 'Alice Smith'"
        ).fetchone()
        assert row is not None
        assert row["AUTHOR_FIRST"] == "Alice"
        assert row["AUTHOR_LAST"] == "Smith"

    def test_two_token_name_bob(self, migrated_db):
        row = migrated_db.execute(
            "SELECT AUTHOR_FIRST, AUTHOR_LAST FROM AUTHOR WHERE AUTHOR_FULL_NAME = 'Bob Jones'"
        ).fetchone()
        assert row is not None
        assert row["AUTHOR_FIRST"] == "Bob"
        assert row["AUTHOR_LAST"] == "Jones"

    def test_single_token_name_gets_null_first(self, migrated_db):
        row = migrated_db.execute(
            "SELECT AUTHOR_FIRST, AUTHOR_LAST FROM AUTHOR WHERE AUTHOR_FULL_NAME = 'Madonna'"
        ).fetchone()
        assert row is not None
        assert row["AUTHOR_FIRST"] is None
        assert row["AUTHOR_LAST"] == "Madonna"

    def test_compound_first_name(self):
        """'Alice Van Smith' → first='Alice Van', last='Smith'."""
        from migrate_db import _split_name
        assert _split_name("Alice Van Smith") == ("Alice Van", "Smith")

    def test_single_token_split_function(self):
        from migrate_db import _split_name
        assert _split_name("Madonna") == (None, "Madonna")

    def test_empty_string_split_function(self):
        from migrate_db import _split_name
        assert _split_name("") == (None, None)

    def test_all_authors_have_last_name(self, migrated_db):
        rows = migrated_db.execute("SELECT AUTHOR_LAST FROM AUTHOR").fetchall()
        for row in rows:
            assert row["AUTHOR_LAST"] is not None


# ---------------------------------------------------------------------------
# DB_VERSION
# ---------------------------------------------------------------------------

class TestDBVersion:
    def test_version_table_exists(self, migrated_db):
        row = migrated_db.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='DB_VERSION'"
        ).fetchone()
        assert row is not None

    def test_version_is_0_1_2(self, migrated_db):
        row = migrated_db.execute(
            "SELECT VERSION FROM DB_VERSION WHERE VERSION = '0.1.2'"
        ).fetchone()
        assert row is not None

    def test_old_db_has_no_version_table(self, old_db):
        conn = sqlite3.connect(old_db)
        row = conn.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='DB_VERSION'"
        ).fetchone()
        conn.close()
        assert row is None  # old schema predates versioning


# ---------------------------------------------------------------------------
# Safety checks (--force guard)
# ---------------------------------------------------------------------------

class TestOrphanNoteWarning:
    def test_orphan_warning_fires_when_orphans_exist(self, old_db, new_db, capsys):
        """A WARNING is printed to stderr when old notes have NULL paper_id."""
        from migrate_db import run_migration
        run_migration(old_db, new_db, force=True)
        captured = capsys.readouterr()
        assert "WARNING" in captured.err
        assert "orphan" in captured.err.lower() or "null paper_id" in captured.err.lower()

    def test_no_orphan_warning_when_no_orphans(self, tmp_path, capsys):
        """No WARNING is printed when all notes have valid paper_ids."""
        from migrate_db import run_migration
        old_path = str(tmp_path / "clean_old.db")
        conn = sqlite3.connect(old_path)
        conn.executescript(_OLD_SCHEMA_SQL)
        conn.execute(
            "INSERT INTO papers (paper_id, version, title, authors, tags, source) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            ("2401.00001", 1, "Clean Paper", json.dumps(["Alice"]), json.dumps([]), "arxiv"),
        )
        conn.execute(
            "INSERT INTO notes (paper_id, title, content) VALUES (?, ?, ?)",
            ("2401.00001", "Clean Note", "No orphans here"),
        )
        conn.commit()
        conn.close()

        new_path = str(tmp_path / "clean_new.db")
        run_migration(old_path, new_path, force=True)
        captured = capsys.readouterr()
        assert "WARNING" not in captured.err


class TestSafetyGuards:
    def test_refuses_existing_db_without_force(self, old_db, new_db):
        Path(new_db).touch()
        from migrate_db import run_migration
        with pytest.raises(SystemExit) as exc:
            run_migration(old_db, new_db, force=False)
        assert exc.value.code == 1

    def test_force_overwrites_existing_db(self, old_db, new_db):
        Path(new_db).write_text("not a db")
        from migrate_db import run_migration
        run_migration(old_db, new_db, force=True)
        conn = sqlite3.connect(new_db)
        count = conn.execute("SELECT COUNT(*) FROM PAPER").fetchone()[0]
        conn.close()
        assert count == 4  # v1+v2 arxiv, v1 openalex, v1 null-source

    def test_refuses_missing_old_db(self, new_db):
        from migrate_db import run_migration
        with pytest.raises(SystemExit) as exc:
            run_migration("/nonexistent/path/old.db", new_db, force=True)
        assert exc.value.code == 1


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------

class TestEdgeCases:
    def test_empty_old_db(self, tmp_path):
        """Migration of an empty old DB should produce an empty new DB without error."""
        from migrate_db import run_migration
        old_path = str(tmp_path / "empty_old.db")
        conn = sqlite3.connect(old_path)
        conn.executescript(_OLD_SCHEMA_SQL)
        conn.commit()
        conn.close()

        new_path = str(tmp_path / "empty_new.db")
        run_migration(old_path, new_path, force=True)

        conn = sqlite3.connect(new_path)
        assert conn.execute("SELECT COUNT(*) FROM PAPER").fetchone()[0] == 0
        assert conn.execute("SELECT COUNT(*) FROM PAPER_ROOTS").fetchone()[0] == 0
        assert conn.execute("SELECT COUNT(*) FROM PROJECT").fetchone()[0] == 0
        conn.close()

    def test_paper_with_no_authors(self, tmp_path):
        """Papers with empty authors list should not insert any AUTHOR rows for them."""
        from migrate_db import run_migration
        old_path = str(tmp_path / "noauth_old.db")
        conn = sqlite3.connect(old_path)
        conn.executescript(_OLD_SCHEMA_SQL)
        conn.execute(
            "INSERT INTO papers (paper_id, version, title, authors, tags, source) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            ("2401.99999", 1, "Anon Paper", json.dumps([]), json.dumps([]), "arxiv"),
        )
        conn.commit()
        conn.close()

        new_path = str(tmp_path / "noauth_new.db")
        run_migration(old_path, new_path, force=True)

        conn = sqlite3.connect(new_path)
        assert conn.execute("SELECT COUNT(*) FROM AUTHOR").fetchone()[0] == 0
        assert conn.execute("SELECT COUNT(*) FROM PAPER_TO_AUTHOR").fetchone()[0] == 0
        conn.close()

    def test_paper_with_no_tags(self, tmp_path):
        """Papers with empty tags list should not insert any TAG rows."""
        from migrate_db import run_migration
        old_path = str(tmp_path / "notag_old.db")
        conn = sqlite3.connect(old_path)
        conn.executescript(_OLD_SCHEMA_SQL)
        conn.execute(
            "INSERT INTO papers (paper_id, version, title, authors, tags, source) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            ("2401.99998", 1, "Untagged", json.dumps(["Author One"]), json.dumps([]), "arxiv"),
        )
        conn.commit()
        conn.close()

        new_path = str(tmp_path / "notag_new.db")
        run_migration(old_path, new_path, force=True)

        conn = sqlite3.connect(new_path)
        assert conn.execute("SELECT COUNT(*) FROM TAG").fetchone()[0] == 0
        assert conn.execute("SELECT COUNT(*) FROM PAPER_TO_TAG").fetchone()[0] == 0
        conn.close()

    def test_project_preserves_status(self, migrated_db):
        row = migrated_db.execute("SELECT STATUS FROM PROJECT WHERE PROJECT_FK = 1").fetchone()
        assert row is not None
        assert row["STATUS"] == "active"

    def test_source_ids_are_namespaced(self, migrated_db):
        """All SOURCE_IDs in PAPER_ROOTS must contain a ':' namespace prefix."""
        rows = migrated_db.execute("SELECT SOURCE_ID FROM PAPER_ROOTS").fetchall()
        for row in rows:
            assert ":" in row["SOURCE_ID"], f"SOURCE_ID {row['SOURCE_ID']!r} is not namespaced"

    def test_arxiv_source_id_prefix(self, migrated_db):
        row = migrated_db.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_ID = 'arxiv:2401.00001'"
        ).fetchone()
        assert row is not None

    def test_openalex_source_id_prefix(self, migrated_db):
        row = migrated_db.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_ID = 'openalex:W3123456789'"
        ).fetchone()
        assert row is not None

    def test_null_source_gets_linxiv_prefix(self, migrated_db):
        row = migrated_db.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_ID = 'linxiv:manual001'"
        ).fetchone()
        assert row is not None

    def test_paper_version_ordering(self, migrated_db):
        rows = migrated_db.execute(
            "SELECT VERSION FROM PAPER WHERE SOURCE_ID = 'arxiv:2401.00001' ORDER BY VERSION"
        ).fetchall()
        versions = [r["VERSION"] for r in rows]
        assert versions == [1, 2]

    def test_fts_searchable(self, migrated_db):
        results = migrated_db.execute(
            "SELECT paper_id FROM papers_fts WHERE papers_fts MATCH 'transformers'"
        ).fetchall()
        assert len(results) == 1

    def test_fts_join_matches_source_id(self, migrated_db):
        """The search_full_text JOIN (p.source_id = fts.paper_id) must resolve after migration.
        Multiple PAPER versions sharing a SOURCE_ID all match — that's expected behavior.
        """
        results = migrated_db.execute("""
            SELECT DISTINCT p.SOURCE_ID FROM PAPER p
            JOIN papers_fts fts ON p.SOURCE_ID = fts.paper_id
            WHERE papers_fts MATCH 'transformers'
        """).fetchall()
        assert len(results) == 1
        assert results[0]["SOURCE_ID"] == "arxiv:2401.00001"

    def test_author_index_preserved(self, migrated_db):
        """First author of paper1 v1 should have AUTHOR_INDEX = 0."""
        paper_row = migrated_db.execute(
            "SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:2401.00001' AND VERSION = 1"
        ).fetchone()
        pta_row = migrated_db.execute(
            "SELECT AUTHOR_INDEX FROM PAPER_TO_AUTHOR "
            "WHERE PAPER_ID = ? ORDER BY AUTHOR_INDEX LIMIT 1",
            (paper_row["PAPER_ID"],),
        ).fetchone()
        assert pta_row["AUTHOR_INDEX"] == 0

    def test_note_linked_to_correct_paper(self, migrated_db):
        note = migrated_db.execute("SELECT SOURCE_FK FROM NOTE").fetchone()
        root = migrated_db.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_FK = ?",
            (note["SOURCE_FK"],),
        ).fetchone()
        assert root["SOURCE_ID"] == "arxiv:2401.00001"

    def test_project_tags_in_junction_table(self, migrated_db):
        """Project tags must be in PROJECT_TO_TAG (PROJECT.PROJECT_TAGS column was removed)."""
        tag_row = migrated_db.execute(
            "SELECT t.TAG FROM PROJECT_TO_TAG pt "
            "JOIN TAG t ON t.TAG_FK = pt.TAG_FK "
            "WHERE pt.PROJECT_FK = 1"
        ).fetchone()
        assert tag_row is not None
        assert tag_row["TAG"] == "ml"

    def test_paper_meta_fields_carried_over(self, migrated_db):
        """Key PAPER_META columns must be copied from the old papers table."""
        paper = migrated_db.execute(
            "SELECT PAPER_ID FROM PAPER WHERE SOURCE_ID = 'arxiv:2401.00001' AND VERSION = 1"
        ).fetchone()
        meta = migrated_db.execute(
            "SELECT PROVIDER, FULL_TEXT, DOWNLOADED_SOURCE FROM PAPER_META WHERE PAPER_ID = ?",
            (paper["PAPER_ID"],),
        ).fetchone()
        assert meta["PROVIDER"] == "arxiv"
        assert meta["FULL_TEXT"] == "This paper studies transformers."
        assert meta["DOWNLOADED_SOURCE"] == 0

    def test_fts_paper_id_column_is_source_id(self, migrated_db):
        """papers_fts.paper_id must hold SOURCE_ID (text like '2401.00001') so that
        search_full_text's JOIN p.source_id = fts.PAPER_ID resolves correctly."""
        row = migrated_db.execute("SELECT paper_id FROM papers_fts LIMIT 1").fetchone()
        assert row is not None
        # Must be a text source_id, not an integer string
        source_id = row["paper_id"]
        root = migrated_db.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_ID = ?", (source_id,)
        ).fetchone()
        assert root is not None, f"FTS paper_id {source_id!r} not found in PAPER_ROOTS"

    def test_crossref_source_gets_doi_prefix(self, tmp_path):
        """Papers with source='crossref' get 'doi:' prefix."""
        from migrate_db import run_migration
        old_path = str(tmp_path / "cr_old.db")
        conn = sqlite3.connect(old_path)
        conn.executescript(_OLD_SCHEMA_SQL)
        conn.execute(
            "INSERT INTO papers (paper_id, version, title, authors, tags, source) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            ("10.1145/12345", 1, "CrossRef Paper", json.dumps(["A"]), json.dumps([]), "crossref"),
        )
        conn.commit()
        conn.close()

        new_path = str(tmp_path / "cr_new.db")
        run_migration(old_path, new_path, force=True)
        conn = sqlite3.connect(new_path)
        row = conn.execute(
            "SELECT SOURCE_ID FROM PAPER_ROOTS WHERE SOURCE_ID = 'doi:10.1145/12345'"
        ).fetchone()
        conn.close()
        assert row is not None

    def test_note_with_nonexistent_paper_id_silently_dropped(self, tmp_path):
        """Notes whose paper_id doesn't exist in old.papers are silently skipped (not in orphan count)."""
        from migrate_db import run_migration
        old_path = str(tmp_path / "dangling_old.db")
        conn = sqlite3.connect(old_path)
        conn.executescript(_OLD_SCHEMA_SQL)
        conn.execute(
            "INSERT INTO papers (paper_id, version, title, authors, tags, source) "
            "VALUES (?, ?, ?, ?, ?, ?)",
            ("2401.00001", 1, "Real Paper", json.dumps(["Alice"]), json.dumps([]), "arxiv"),
        )
        # Note referencing a paper that doesn't exist in old.papers
        conn.execute(
            "INSERT INTO notes (paper_id, title, content) VALUES (?, ?, ?)",
            ("9999.99999", "Dangling Note", "References nonexistent paper"),
        )
        conn.commit()
        conn.close()

        new_path = str(tmp_path / "dangling_new.db")
        run_migration(old_path, new_path, force=True)

        conn = sqlite3.connect(new_path)
        # The dangling note is dropped (JOIN with PAPER_ROOTS fails)
        assert conn.execute("SELECT COUNT(*) FROM NOTE").fetchone()[0] == 0
        conn.close()


# ---------------------------------------------------------------------------
# Integration test against the real v0.1.0 DB
# ---------------------------------------------------------------------------

@pytest.mark.skipif(not _REAL_OLD_DB.exists(), reason="real old DB not present")
class TestRealDBMigration:
    @pytest.fixture()
    def migrated_real_db(self, tmp_path):
        from migrate_db import run_migration
        new_path = str(tmp_path / "new_real.db")
        run_migration(str(_REAL_OLD_DB), new_path, force=True)
        conn = sqlite3.connect(new_path)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA foreign_keys = ON")
        yield conn
        conn.close()

    def test_all_source_ids_are_namespaced(self, migrated_real_db):
        rows = migrated_real_db.execute("SELECT SOURCE_ID FROM PAPER_ROOTS").fetchall()
        assert len(rows) > 0, "Expected at least one paper in old DB"
        for row in rows:
            assert ":" in row["SOURCE_ID"], f"Not namespaced: {row['SOURCE_ID']!r}"

    def test_fk_integrity(self, migrated_real_db):
        violations = migrated_real_db.execute("PRAGMA foreign_key_check").fetchall()
        assert list(violations) == [], f"FK violations: {[dict(r) for r in violations]}"

    def test_version_is_0_1_2(self, migrated_real_db):
        row = migrated_real_db.execute(
            "SELECT VERSION FROM DB_VERSION WHERE VERSION = '0.1.2'"
        ).fetchone()
        assert row is not None

    def test_paper_roots_matches_distinct_papers(self, migrated_real_db):
        old_conn = sqlite3.connect(str(_REAL_OLD_DB))
        old_count = old_conn.execute(
            "SELECT COUNT(DISTINCT paper_id) FROM papers WHERE paper_id IS NOT NULL"
        ).fetchone()[0]
        old_conn.close()
        new_count = migrated_real_db.execute("SELECT COUNT(*) FROM PAPER_ROOTS").fetchone()[0]
        assert new_count == old_count

    def test_paper_rows_match(self, migrated_real_db):
        old_conn = sqlite3.connect(str(_REAL_OLD_DB))
        old_count = old_conn.execute("SELECT COUNT(*) FROM papers").fetchone()[0]
        old_conn.close()
        new_count = migrated_real_db.execute("SELECT COUNT(*) FROM PAPER").fetchone()[0]
        assert new_count == old_count

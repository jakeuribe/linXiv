from __future__ import annotations

import sqlite3
from pathlib import Path

from config import resources_dir as _resources_dir
from storage.paths import db_path as _db_path_fn

_SQL_DIR = _resources_dir() / "storage" / "config" / "sql"
_TABLES_DIR = _SQL_DIR / "tables"
_VIEWS_DIR = _SQL_DIR / "views"

# Apply in dependency order (FK-safe). Versioned rows: ``PAPER`` + ``PAPER_META``; root id in ``paper_roots``.
_TABLE_DDL_ORDER: tuple[str, ...] = (
    "AUTHOR.sql",
    "TAG.sql",
    "PROJECT.sql",
    "PAPER_ROOTS.sql",
    "PAPER.sql",
    "PAPER_META.sql",
    "PAPER_TO_AUTHOR.sql",
    "PAPER_TO_TAG.sql",
    "PROJECT_TO_PAPER.sql",
    "PROJECT_TO_TAG.sql",
    "NOTE.sql",
    "papers_fts.sql",
    "DB_VERSION.sql",
    "SEARCH_HISTORY.sql",
    "SEARCH_STATE.sql",
)

DB_PATH = str(_db_path_fn())


def _ordered_table_paths() -> list[Path]:
    paths: list[Path] = []
    for name in _TABLE_DDL_ORDER:
        p = _TABLES_DIR / name
        if p.is_file():
            paths.append(p)
    return paths


def _apply_views(conn: sqlite3.Connection) -> None:
    if not _VIEWS_DIR.is_dir():
        return
    candidates = sorted(_VIEWS_DIR.glob("*.sql")) + sorted(_VIEWS_DIR.glob("*.SQL"))
    for path in candidates:
        script = path.read_text(encoding="utf-8").strip()
        if script:
            conn.executescript(script)


def _apply_indices(conn: sqlite3.Connection) -> None:
    path = _SQL_DIR / "INDICIES.SQL"
    if not path.is_file():
        return
    script = path.read_text(encoding="utf-8").strip()
    if script:
        conn.executescript(script)


def _migrate_paper_roots_soft_delete(conn: sqlite3.Connection) -> None:
    cols = {row[1] for row in conn.execute("PRAGMA table_info(PAPER_ROOTS)")}
    if "STATUS" not in cols:
        conn.execute(
            "ALTER TABLE PAPER_ROOTS ADD COLUMN STATUS TEXT NOT NULL DEFAULT 'active'"
        )
    if "DELETED_AT" not in cols:
        conn.execute("ALTER TABLE PAPER_ROOTS ADD COLUMN DELETED_AT TIMESTAMP")


def _migrate_paper_meta_provider(conn: sqlite3.Connection) -> None:
    cols = {row[1] for row in conn.execute("PRAGMA table_info(PAPER_META)")}
    if "PROVIDER" not in cols:
        conn.execute(
            "ALTER TABLE PAPER_META ADD COLUMN PROVIDER TEXT DEFAULT 'arxiv'"
        )


def _migrate_search_state_sort_json(conn: sqlite3.Connection) -> None:
    cols = {row[1] for row in conn.execute("PRAGMA table_info(SEARCH_STATE)")}
    if "SORT_JSON" not in cols:
        conn.execute("ALTER TABLE SEARCH_STATE ADD COLUMN SORT_JSON TEXT")


def _migrate_tag_label_unique_index(conn: sqlite3.Connection) -> None:
    idx = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_tag_label_unique'"
    ).fetchone()
    if idx:
        return
    # Build canonical FK map (min TAG_FK per lowercase label) and collect non-canonical rows
    # in a single pass over the TAG table.
    canonical: dict[str, int] = {}
    tag_rows = conn.execute("SELECT TAG_FK, TAG FROM TAG ORDER BY TAG_FK").fetchall()
    for row in tag_rows:
        key = row["TAG"].lower()
        if key not in canonical:
            canonical[key] = row["TAG_FK"]
    # Remap FK references from non-canonical to canonical, then delete non-canonical TAG rows.
    # UPDATE OR IGNORE suppresses interim duplicates in PROJECT_TO_TAG; these will be cleaned
    # by the subsequent _migrate_project_to_tag_unique_index pass. DELETE removes any
    # PROJECT_TO_TAG rows that could not be remapped (e.g., canonical link already existed).
    remapped = False
    for row in tag_rows:
        canon_fk = canonical.get(row["TAG"].lower())
        if canon_fk is not None and row["TAG_FK"] != canon_fk:
            remapped = True
            old_fk = row["TAG_FK"]
            conn.execute(
                "UPDATE OR IGNORE PROJECT_TO_TAG SET TAG_FK = ? WHERE TAG_FK = ?",
                (canon_fk, old_fk),
            )
            conn.execute("DELETE FROM PROJECT_TO_TAG WHERE TAG_FK = ?", (old_fk,))
            conn.execute(
                "UPDATE OR IGNORE PAPER_TO_TAG SET TAG_FK = ? WHERE TAG_FK = ?",
                (canon_fk, old_fk),
            )
            conn.execute("DELETE FROM PAPER_TO_TAG WHERE TAG_FK = ?", (old_fk,))
            conn.execute("DELETE FROM TAG WHERE TAG_FK = ?", (old_fk,))
    # Remove duplicate paper-tag links that the remapping may have introduced.
    # Only runs when remapping actually happened; skipped on the common no-op path.
    if remapped:
        conn.execute(
            """
            DELETE FROM PAPER_TO_TAG
            WHERE PTT_FK NOT IN (
                SELECT MIN(PTT_FK) FROM PAPER_TO_TAG GROUP BY PAPER_ID, TAG_FK
            )
            """
        )
    conn.execute("CREATE UNIQUE INDEX idx_tag_label_unique ON TAG (TAG COLLATE NOCASE)")


def _migrate_project_to_tag_unique_index(conn: sqlite3.Connection) -> None:
    idx = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_project_to_tag_unique'"
    ).fetchone()
    if idx:
        return
    # Deduplicate before creating index: keep lowest PK per (PROJECT_FK, TAG_FK) pair.
    # _migrate_tag_label_unique_index runs first and may introduce duplicates here when
    # remapping non-canonical TAG_FK values.
    conn.execute(
        """
        DELETE FROM PROJECT_TO_TAG
        WHERE PROJECT_TO_TAG_FK NOT IN (
            SELECT MIN(PROJECT_TO_TAG_FK)
            FROM PROJECT_TO_TAG
            GROUP BY PROJECT_FK, TAG_FK
        )
        """
    )
    conn.execute(
        "CREATE UNIQUE INDEX idx_project_to_tag_unique "
        "ON PROJECT_TO_TAG (PROJECT_FK, TAG_FK)"
    )


def _migrate_project_to_paper_unique_index(conn: sqlite3.Connection) -> None:
    idx = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_project_to_paper_unique'"
    ).fetchone()
    if idx:
        return
    # Deduplicate before creating index: keep lowest PK per (PROJECT_FK, SOURCE_FK)
    # pair. Duplicates could exist from the pre-index era when membership writes
    # were unconstrained.
    conn.execute(
        """
        DELETE FROM PROJECT_TO_PAPER
        WHERE PROJECT_TO_PAPER_FK NOT IN (
            SELECT MIN(PROJECT_TO_PAPER_FK)
            FROM PROJECT_TO_PAPER
            GROUP BY PROJECT_FK, SOURCE_FK
        )
        """
    )
    conn.execute(
        "CREATE UNIQUE INDEX idx_project_to_paper_unique "
        "ON PROJECT_TO_PAPER (PROJECT_FK, SOURCE_FK)"
    )


def _migrate_author_full_name_index(conn: sqlite3.Connection) -> None:
    # Indexes AUTHOR_FULL_NAME under COLLATE NOCASE for the graph's author-name
    # join. Non-unique: create_author does not dedup names.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_author_full_name "
        "ON AUTHOR (AUTHOR_FULL_NAME COLLATE NOCASE)"
    )


def apply_sql_schema(conn: sqlite3.Connection) -> None:
    """Create bundled tables (and optional views/indexes) from ``sql/tables``."""
    conn.execute("PRAGMA foreign_keys = ON")
    for path in _ordered_table_paths():
        script = path.read_text(encoding="utf-8").strip()
        if script:
            conn.executescript(script)
    _migrate_paper_roots_soft_delete(conn)
    _migrate_paper_meta_provider(conn)
    _migrate_search_state_sort_json(conn)
    _migrate_tag_label_unique_index(conn)
    _migrate_project_to_tag_unique_index(conn)
    _migrate_project_to_paper_unique_index(conn)
    _migrate_author_full_name_index(conn)
    _apply_views(conn)
    _apply_indices(conn)
    # Persist regardless of how the caller manages the connection: not every
    # caller wraps this in a committing context (e.g. init_db closes directly),
    # and sqlite3 does not auto-commit an open transaction before DDL.
    conn.commit()


def init_db(db_path: str | None = None) -> None:
    """Standalone initializer: open ``papers.db`` (or ``db_path``) and apply SQL."""
    path = db_path if db_path else DB_PATH
    conn = sqlite3.connect(path)
    try:
        apply_sql_schema(conn)
    finally:
        conn.close()

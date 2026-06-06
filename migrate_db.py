"""One-time schema migration: old (blue) DB -> new (green) DB.

Usage:
    python migrate_db.py old_papers.db [--new-db papers.db] [--force]
    linxiv-migrate old_papers.db [--new-db papers.db] [--force]
"""
from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

from storage.config.core import apply_sql_schema
from storage.paths import db_path as _default_new_db_path
from config import init_data_dir

_MIGRATE_SQL = Path(__file__).resolve().parent / "storage" / "migrations" / "migrate_data.sql"


def _connect(path: str) -> sqlite3.Connection:
    conn = sqlite3.connect(path, detect_types=sqlite3.PARSE_DECLTYPES)
    conn.row_factory = sqlite3.Row
    return conn


def _split_name(full: str) -> tuple[str | None, str | None]:
    """Split 'First Last' into (first, last) on the rightmost whitespace.

    Single-token names (no space) return (None, token). Empty string → (None, None).
    """
    parts = full.rsplit(None, 1)
    if len(parts) == 0:
        return None, None
    if len(parts) == 1:
        return None, parts[0]
    return parts[0], parts[1]


def _verify(conn: sqlite3.Connection) -> bool:
    """Log row-count comparisons between old and new DB. Returns True if all match."""
    checks = [
        ("papers (all rows)",       "SELECT COUNT(*) FROM old.papers",                      "SELECT COUNT(*) FROM PAPER"),
        ("paper roots",             "SELECT COUNT(DISTINCT paper_id) FROM old.papers",       "SELECT COUNT(*) FROM PAPER_ROOTS"),
        ("projects",                "SELECT COUNT(*) FROM old.projects",                     "SELECT COUNT(*) FROM PROJECT"),
        ("notes (with paper_id)",   "SELECT COUNT(*) FROM old.notes WHERE paper_id IS NOT NULL", "SELECT COUNT(*) FROM NOTE"),
    ]
    all_ok = True
    for label, old_sql, new_sql in checks:
        old_n = conn.execute(old_sql).fetchone()[0]
        new_n = conn.execute(new_sql).fetchone()[0]
        status = "OK" if old_n == new_n else "MISMATCH"
        if status == "MISMATCH":
            all_ok = False
        print(f"[migrate] {status}: {label} old={old_n} new={new_n}")
    return all_ok


def run_migration(old_db: str, new_db: str, *, force: bool = False) -> None:
    new_path = Path(new_db)

    if new_path.exists() and not force:
        print(f"[migrate] {new_db!r} already exists. Pass --force to overwrite.", file=sys.stderr)
        sys.exit(1)

    if new_path.exists() and force:
        new_path.unlink()
        print(f"[migrate] Removed existing {new_db!r}")

    if not Path(old_db).exists():
        print(f"[migrate] Old DB not found: {old_db!r}", file=sys.stderr)
        sys.exit(1)

    if not _MIGRATE_SQL.exists():
        print(f"[migrate] migrate_data.sql not found at {_MIGRATE_SQL}", file=sys.stderr)
        sys.exit(1)

    conn = None
    try:
        # 1. Apply green schema DDL (includes DB_VERSION with v0.1.1 seed row)
        print(f"[migrate] Applying schema to {new_db!r}...")
        conn = _connect(new_db)
        apply_sql_schema(conn)

        # 2. Attach old DB and run migrate_data.sql
        print(f"[migrate] Attaching old DB {old_db!r}...")
        conn.execute("ATTACH DATABASE ? AS old", (old_db,))

        print("[migrate] Running migration SQL...")
        sql = _MIGRATE_SQL.read_text(encoding="utf-8")
        conn.executescript(sql)

        # 3. Post-process AUTHOR: fill AUTHOR_FIRST / AUTHOR_LAST via Python split.
        #    The SQL leaves these NULL because SQLite has no reverse()/rinstr().
        print("[migrate] Splitting author names...")
        with conn:
            rows = conn.execute(
                "SELECT AUTHOR_FK, AUTHOR_FULL_NAME FROM AUTHOR "
                "WHERE AUTHOR_FIRST IS NULL AND AUTHOR_LAST IS NULL"
            ).fetchall()
            for row in rows:
                first, last = _split_name(row["AUTHOR_FULL_NAME"] or "")
                conn.execute(
                    "UPDATE AUTHOR SET AUTHOR_FIRST = ?, AUTHOR_LAST = ? WHERE AUTHOR_FK = ?",
                    (first, last, row["AUTHOR_FK"]),
                )

        # 4. Surface orphan notes (NULL paper_id — cannot be migrated)
        try:
            orphan_row = conn.execute("SELECT n FROM _orphan_notes_count").fetchone()
            orphan_count = int(orphan_row[0]) if orphan_row else 0
        except Exception:
            orphan_count = 0
        if orphan_count:
            print(
                f"[migrate] WARNING: {orphan_count} note(s) had NULL paper_id and could not be "
                "migrated (SOURCE_FK is NOT NULL in new schema).",
                file=sys.stderr,
            )

        # 5. Verify row counts
        print("[migrate] Verifying row counts...")
        _verify(conn)

        # 6. Confirm schema version
        version_row = conn.execute(
            "SELECT VERSION FROM DB_VERSION ORDER BY VERSION_FK DESC LIMIT 1"
        ).fetchone()
        print(f"[migrate] Schema version: {version_row[0] if version_row else 'unknown'}")

    except SystemExit:
        raise
    except Exception as e:
        print(f"[migrate] Fatal error: {e}", file=sys.stderr)
        sys.exit(1)
    finally:
        if conn is not None:
            conn.close()

    print("[migrate] Done.")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="linxiv-migrate",
        description="Migrate linXiv DB from old (blue) to new (green) schema.",
    )
    p.add_argument("old_db", help="Path to old SQLite DB")
    p.add_argument(
        "--new-db", default=None, dest="new_db",
        help="Destination path for new DB (default: $LINXIV_DATA_DIR/papers.db)",
    )
    p.add_argument(
        "--force", action="store_true", default=False,
        help="Overwrite an existing new DB",
    )
    return p


def main(argv: list[str] | None = None) -> None:
    init_data_dir()
    parser = build_parser()
    args = parser.parse_args(argv)
    new_db = args.new_db or str(_default_new_db_path())
    run_migration(args.old_db, new_db, force=args.force)


if __name__ == "__main__":
    main()

"""Storage layer — SQLite database and notes."""

from .db import (
    DB_PATH,
    _connect,
    init_db,
    init_table,
    parse_entry_id,
    save_paper,
    save_papers,
    save_paper_metadata,
    save_papers_metadata,
    get_paper,
    get_all_versions,
    delete_paper,
    list_papers,
    get_categories,
    get_tags,
    get_graph_data,
    set_has_pdf,
    set_pdf_path,
    set_full_text,
    search_full_text,
)
from .notes import (
    Note,
    ensure_notes_db,
    init_notes_db,
    get_note,
    get_notes,
    get_project_notes,
    count_project_notes,
    count_paper_notes,
)

__all__ = [
    # db
    "DB_PATH", "_connect", "init_db", "init_table", "parse_entry_id",
    "save_paper", "save_papers", "save_paper_metadata", "save_papers_metadata",
    "get_paper", "get_all_versions", "delete_paper", "list_papers",
    "get_categories", "get_tags", "get_graph_data",
    "set_has_pdf", "set_pdf_path", "set_full_text", "search_full_text",
    # notes
    "Note", "ensure_notes_db", "init_notes_db",
    "get_note", "get_notes", "get_project_notes",
    "count_project_notes", "count_paper_notes",
]

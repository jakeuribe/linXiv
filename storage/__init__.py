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
    get_paper_by_id,
    get_paper_by_source_fk,
    get_paper_root,
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
from .projects import (
    Q,
    Status,
    Project,
    ensure_projects_db,
    init_projects_db,
    get_project,
    filter_projects,
    color_to_hex,
    color_from_hex,
)
from .notes import (
    Note,
    ensure_notes_db,
    get_note,
    get_notes,
    get_project_notes,
    count_project_notes,
    count_paper_notes,
    note_counts_by_paper_for_project,
)

__all__ = [
    # db
    "DB_PATH", "_connect", "init_db", "init_table", "parse_entry_id",
    "save_paper", "save_papers", "save_paper_metadata", "save_papers_metadata",
    "get_paper", "get_paper_by_id", "get_paper_by_source_fk", "get_paper_root",
    "get_all_versions", "delete_paper", "list_papers",
    "get_categories", "get_tags", "get_graph_data",
    "set_has_pdf", "set_pdf_path", "set_full_text", "search_full_text",
    # projects
    "Q", "Status", "Project", "ensure_projects_db", "init_projects_db",
    "get_project", "filter_projects", "color_to_hex", "color_from_hex",
    # notes
    "Note", "ensure_notes_db",
    "get_note", "get_notes", "get_project_notes",
    "count_project_notes", "count_paper_notes",
    "note_counts_by_paper_for_project",
]

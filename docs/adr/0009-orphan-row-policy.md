# ADR 0009: Orphan row policy — AUTHOR, TAG, and NOTE on paper/project delete

## Status

Accepted

## Context

Three types of rows can become orphaned when papers or projects are deleted:

1. **AUTHOR rows** — rows in the `AUTHOR` lookup table that are no longer referenced by any `PAPER_TO_AUTHOR` row (i.e. no paper in the library uses that author name).
2. **TAG rows** — rows in the `TAG` lookup table that are no longer referenced by any `PAPER_TO_TAG` or `PROJECT_TO_TAG` row.
3. **NOTE rows** — rows in the `NOTE` table whose `PROJECT_FK` references a deleted project.

The schema uses `ON DELETE CASCADE` for structural child relationships (PAPER → PAPER_META, PAPER → PAPER_TO_AUTHOR, PAPER_ROOTS → PAPER, etc.). However, CASCADE cannot handle orphan cleanup in lookup tables (AUTHOR, TAG) because lookup tables are *parents* of junction tables, not children — CASCADE only flows from parent to child.

Three operations create orphan risk:
- `db.repair_paper` changes a paper's author list.
- `db.hard_delete_paper` removes a paper entirely.
- `service.project.hard_delete` removes a project entirely.

## Decision

### AUTHOR rows

**On repair:** `_sync_paper_authors` cleans up orphaned AUTHOR rows after each author-list sync. When authors are removed from a paper, any AUTHOR row no longer referenced by any other paper is deleted. This prevents the AUTHOR lookup table from accumulating stale entries as papers are corrected over time.

**On hard delete:** `hard_delete_paper` relies solely on schema CASCADE to remove `PAPER_TO_AUTHOR` rows when `PAPER_ROOTS` is deleted. It does not call `_sync_paper_authors` and does not clean up orphaned AUTHOR rows. The AUTHOR rows remain as inert entries in the lookup table.

**Accepted trade-off:** AUTHOR orphans on hard delete are allowed. The cleanup code in `_sync_paper_authors` is tied to the repair workflow where author names change incrementally. Calling it during hard delete would require extracting a helper and changing the deletion transaction — added complexity for a low-priority housekeeping win. An orphaned AUTHOR row has no functional consequence: it never appears in the UI and wastes negligible space. Introduce orphan cleanup if usecase of linxiv changes, but vanishingly small consequences for current usecases.

### TAG rows

TAG orphans are never cleaned up automatically — not on repair, not on hard delete, not on project delete. `_sync_paper_tags` replaces a paper's tag associations but does not delete TAG rows from the lookup table. `hard_delete_project` deletes `PROJECT_TO_TAG` rows but not the TAG entries.

**Accepted trade-off:** TAG rows are shared vocabulary across papers and projects. A TAG that is no longer used by any paper or project is still a valid user-defined label that may be reused in the future. Aggressive cleanup risks deleting a tag a user intended to keep. Orphan TAG rows have no functional consequence and waste negligible space. Same as author, will be revisited for edge cases.

### NOTE rows on project delete

<!-- #TODO: revisit this policy — the NULL-on-delete behaviour below is to be slightly altered. -->

When a project is hard-deleted, `hard_delete_project` does **not** delete NOTE rows that were scoped to that project. Instead, it sets `NOTE.PROJECT_FK = NULL` on those rows. The note content is preserved; the note loses its project scope and becomes a global (unscoped) note.

**Rationale:** A note represents the user's research work. Deleting a project should not silently destroy notes the user may have written. The user can always delete specific notes manually. This is distinct from soft-delete (where the project is recoverable) — even on hard delete, we preserve the intellectual content.

### PROJECT_TO_PAPER rows on paper delete

**On soft-delete:** `service.paper.delete` calls `db.soft_delete_paper`, which marks the paper deleted but does not touch `PROJECT_TO_PAPER`. Project membership is intentionally preserved so that if the paper is restored from trash, it returns to all its projects automatically.

**On hard-delete:** `db.hard_delete_paper` deletes from `PAPER_ROOTS`. Schema CASCADE removes all child rows including `PROJECT_TO_PAPER`. No explicit code needed.

**On restore — optional project removal:** The restore endpoint (`GET /api/trash/papers/{source_id}/restore`) returns `project_fks` in its response so the frontend can present a "keep in projects?" prompt. If the user declines, the frontend calls `DELETE /api/projects/{id}/papers/{source_id}` for each returned FK — no dedicated "remove from all projects" backend endpoint is required. The legacy PyQt GUI had inline code for this; that code has been removed as the GUI is replaced by the React frontend.

## Consequences

### Positive
- Repair operations keep the AUTHOR lookup table clean over time.
- Notes survive project deletion; no accidental content loss.
- TAG vocabulary persists; no surprising disappearance of user-defined labels.

### Negative / limits
- AUTHOR table accumulates orphans when papers are hard-deleted. For a large, high-turnover library this could become measurable noise. A periodic cleanup task (or a `VACUUM`-adjacent operation) would address this if needed.
- TAG table is never cleaned up. If the tag vocabulary grows unbounded through tag churn, it would need explicit housekeeping. Current scope does not include a tag management UI (see TODO.md — Project tags UI).
- Notes detached from a deleted project appear as unscoped notes in the library, which may be confusing without UI context. A future UI improvement could mark these with a "from deleted project" indicator.

## References

- `storage/db.py` — `_sync_paper_authors`, `hard_delete_paper`
- `storage/projects.py` — `hard_delete_project`
- `service/project.py` — `hard_delete`
- `storage/db.py` — `_sync_paper_tags`

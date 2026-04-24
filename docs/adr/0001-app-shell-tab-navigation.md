# ADR 0001: App shell tab history and cross-tab “deep return” handoffs

## Status

Accepted

## Context

The desktop shell (`gui/shell.py`) uses a `QStackedWidget` for main areas (Library, Graph, Projects, etc.). Navigation includes:

- **Tab-level history:** when the stack’s current page index changes, the previous index can be pushed onto `_nav_history`; `go_back()` pops and switches back without recording that transition as a new history entry.
- **Cross-tab flows with in-tab drill-down:** for example, opening a **project** from a **paper detail** in Library switches to Projects and must return to the **same paper detail** (not only the Library list) when the user goes back.

That “return to exact sub-view” cannot be inferred from the stack index alone. The implementation therefore uses **explicit, pairwise wiring** in `gui/app_shell.py` and related pages (for example, stashing a paper id on navigate, passing `return_to_library_paper_id` into `ProjectsPage.open_project`, and having Projects call back into Library after `go_back()`). A `QStackedWidget.currentChanged` handler may reset list vs detail state when landing on certain tabs.

This matches the current product scope and keeps behavior obvious in code.

## Decision

We **keep** this approach:

- Shell remains responsible for **which top-level page** is visible and for a **simple index-based back stack**.
- Individual flows that need **restoration of inner UI state** after back continue to be handled with **page-specific fields and signals**, coordinated from `app_shell` (or similar) as needed.

We do **not** introduce a generic router, route table, or structured navigation stack at this time.

## Consequences

### Positive

- Small, readable changes for a single cross-tab flow.
- No extra abstraction to learn or maintain for contributors.
- Fits the current number of tabs and flows.

### Negative / limits

- **Does not scale linearly** with “another tab that opens something elsewhere and must restore a specific sub-view on back.” Each such flow tends to add:

  - more `attach_*` / `take_*` style bridges between pages,
  - more arguments on `open_*` methods,
  - more special cases in the shell or `currentChanged` handlers,

  and increases the risk of ordering bugs (e.g. list reset vs deferred restore).

### Follow-up (when to revisit)

If we add **more tabs** (or more entry points) that repeat the **same pattern** as Library → Projects (leave tab A in a drill-down, open tab B in a drill-down, Back must restore tab A’s drill-down), we should **revisit this decision** and invest in **better routing**, for example:

- a small **navigation frame** stack (stack index + optional restore callback or opaque payload), or
- a **route-like** model per page (`library:list`, `library:paper:<id>`, `projects:project:<id>`, …) with a single place that applies transitions and back,

so cross-tab deep links do not multiply pairwise coupling.

Until that threshold is reached, the current design remains acceptable.

## References

- `gui/shell.py` — `_nav_history`, `_go_to`, `go_back`, `go_to_widget`
- `gui/app_shell.py` — wiring for `navigate_to_project`, `take_paper_id_for_project_return`, `currentChanged` tab behavior
- `gui/library/page.py` — paper detail → project navigation and return paper id capture
- `gui/projects/page.py` — `return_to_library_paper_id`, back handling with shell `go_back()`

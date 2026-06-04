# ADR 0003: Notes live as a tab inside Paper Detail, not a standalone page

## Status

Accepted

## Context

Notes are project-scoped/(project+paper)-scoped markdown blocks attached to a paper. The question was whether to surface note editing as a tab within the Paper Detail view or as a top-level Notes page in the sidebar navigation.

linxiv's core goal is research paper management. Notes are a supporting feature. A richer text editor (block editor, bidirectional links, etc.) is a future goal but is explicitly out of scope until the core is stable — adding it now would introduce unnecessary complexity.

## Decision

Notes are a tab within **Paper Detail** (e.g. Overview | Notes | PDF). The Notes tab contains a simple markdown editor. A scope picker at the top pre-selects the project the user navigated from; it can be changed to any other project the paper belongs to, or set to global (unscoped).

A standalone Notes page and any advanced editor features are deferred until the core app is stable and can be treated as a "plugin" or update.

## Scope picker semantics (precise, intentional behaviour)

The one-line decision above hides several deliberate boundary choices. They are
spelled out here so they are never mistaken for bugs and "fixed" into incorrect
behaviour. **Every fallback to global (unscoped / `project_id = null`) described
below is correct behaviour, not a defect.**

### What "pre-selects the project the user navigated from" precisely means

1. **Navigation context is advisory, never authoritative.** When the user opens
   a paper from a project list, that project id is passed as React Router
   navigation state (`{ fromProjectId }`). It is *only* a hint for the picker's
   default selection. It is **not** a route, never triggers navigation, and is
   never persisted onto the note. `defaultProjectId = null` is a *scope value*
   meaning "Global"; it does not navigate anywhere.

2. **The hint is gated on live membership, every render.** The picker pre-selects
   `fromProjectId` **only if** the paper currently belongs to that project. If
   the paper does not (or no longer) belongs to it, the default falls back to
   **Global**. This is intentional: a note must never be pre-scoped to a project
   the paper is not a member of.

3. **Membership is sourced from active projects only.** The candidate project
   list comes from `listProjects()`, which returns `status = "active"` projects.
   Therefore:
   - **Navigating from an archived project drops the pre-selection** — the
     archived project is not in the active membership list, so the default falls
     back to Global. Correct: archived projects are not offered as scopes for
     *new* notes.
   - **A new note cannot be scoped to an archived project** from this picker.
     This is intentional; un-archive the project first if you need that scope.

4. **Mid-compose membership changes may move an untouched default.** If the user
   opens "Add note" with a project pre-selected but has **not** yet interacted
   with the picker, and the paper's membership in that project is removed
   elsewhere (the projects cache refetches), the default scope shifts to Global.
   Once the user has touched the picker, their choice is sticky and is never
   overridden. Both halves are intentional.

### Scope is fixed at creation

5. **A note's scope is chosen once, at creation, and is immutable thereafter.**
   The update path (`PATCH /api/notes/{id}`) changes title and content only and
   does **not** reassign `PROJECT_FK`. The editor therefore renders scope
   **read-only when editing** an existing note. This is a deliberate backend
   contract, not a missing feature; see ADR 0009 for how scope is detached
   (set to `NULL`) when a project is hard-deleted.

### Displaying scope on notes that outlived their project

6. **A note scoped to a project not in the active membership list is labelled
   "Project-scoped", never "Global".** This covers notes scoped to a project the
   paper was later removed from, or to an archived project. The note still
   carries its real `project_id`; the neutral label avoids the lie that it is an
   unscoped/global note. Only a genuinely `null` `project_id` renders as
   "Global".

## Consequences

### Positive
- Note editing stays in context of the paper being read.
- No extra sidebar entry cluttering the navigation.
- Scope defaults correctly from navigation context with no extra user action in the common case.
- Keeps complexity low while the core feature set is still being built out.

### Negative / limits
- When the richer editor is eventually built, the tab UI will need to be revisited.
- Explicitly choosing to require minor migration and refactoring when text editor
is introduced.

## References

- `src/pages/PaperDetailPage.tsx` — hosts the Notes tab; computes `paperProjects`
  (active-project membership) and the gated `defaultProjectId`.
- `src/components/notes/NoteEditor.tsx` — the scope picker; read-only-on-edit;
  `defaultProjectId` re-apply effect with the `scopeTouched` sticky guard.
- `src/components/notes/NoteCard.tsx` — scope badge ("Global" / project name /
  "Project-scoped" fallback).
- `src/pages/ProjectDetailPage.tsx` — passes `{ fromProjectId }` navigation state.
- ADR 0009 — what happens to a note's scope when its project is hard-deleted.

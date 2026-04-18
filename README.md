# linXiv

A Python application for discovering, managing, and visualizing academic papers from arXiv. Combines a local SQLite database, AI-powered tagging, Obsidian vault integration, and an interactive D3.js network graph — all wrapped in a PyQt6 GUI.

## Features

- **Paper search** — Search arXiv by keyword, fetch by ID, or look up by DOI; results saved to a local SQLite DB with version tracking
- **Interactive graph** — Force-directed D3.js visualization of papers and authors; real-time force controls (gravity, repulsion, link strength)
- **Projects** — Organise papers into projects; add notes per paper scoped to a project; composable SQL query builder (`Q`) for filtering
- **TeX rendering** — KaTeX renders LaTeX math in titles and abstracts inside the search UI
- **PDF viewer** — Native Qt PDF viewer (`QPdfView`) with zoom and page navigation
- **AI tools** — Google Gemini structured output for tag generation, paper summarization, and semantic similarity
- **Obsidian integration** — Auto-generate markdown notes with YAML frontmatter for your vault
- **PDF & TeX downloads** — Batch download PDFs and TeX source tarballs

## Project Structure

```
linXiv/
├── main_shell.py              # Launch full app shell (recommended)
├── run_api.py                 # HTTP API (FastAPI) for external frontends — run_api.bat
├── api/
│   ├── app.py                 # FastAPI routes + /assets/graph (bundled D3 graph for iframe/proxy)
│   └── graph_payload.py       # Graph JSON (tags + projects) for /api/graph
├── db.py                      # SQLite DB: versioned paper storage, graph data queries
├── projects.py                # Projects: data model, Status enum, Q query builder
├── notes.py                   # Notes: per-paper annotations scoped to projects
├── fetch_paper_metadata.py    # arXiv API: fetch, search, save, generate Obsidian notes
├── downloads.py               # PDF and TeX source download helpers
├── AI_tools.py                # Gemini: tag(), summarize(), find_related(); PaperContent input type
├── formats/
│   └── table_format.md        # YAML frontmatter template for Obsidian notes
├── gui/
│   ├── app_shell.py           # QApplication + AppShell wiring (run via main_shell.py)
│   ├── shell.py               # AppShell: sidebar nav + QStackedWidget page container
│   ├── home_page.py           # Home: stat cards, recent papers list
│   ├── graph_page.py          # Graph page (embedded in shell)
│   ├── projects_page.py       # Projects: list, detail view, add paper/note dialogs
│   ├── doi_page.py            # Add by DOI: three-strategy resolution + save to library
│   ├── setup_page.py          # Setup: API key instructions and status
│   ├── search_window.py       # Floating search UI: tri-pane with TeX rendering and PDF button
│   ├── graph_view.py          # QWebEngineView wrapper for D3 graph
│   ├── tex_view.py            # QWebEngineView wrapper for KaTeX rendering
│   ├── pdf_window.py          # QPdfView PDF viewer with toolbar
│   └── web/
│       ├── graph.html/js/css  # D3 force-directed graph
│       ├── d3.v7.min.js       # Bundled D3
│       ├── tex_view.html      # KaTeX auto-render template
│       ├── katex.min.js/css   # Bundled KaTeX
│       ├── auto-render.min.js # KaTeX auto-render extension
│       └── fonts/             # KaTeX WOFF2 fonts (offline)
├── obsidian_vault/
│   └── arXivVault/            # Generated markdown notes (gitignored)
└── pdfs/                      # Downloaded PDFs (gitignored)
```

## Setup

### Prerequisites

- Python 3.10+
- PyQt6 with WebEngine and PDF support (`PyQt6-WebEngine`, `PyQt6` ≥ 6.4)

### Install dependencies

```bash
pip install -r requirements.txt
```

(Includes PyQt6 for the desktop app and FastAPI/uvicorn for the HTTP API.)

### Environment variables

Create a `.env` file in the project root:

```env
GENAI_API_KEY_TAG_GEN=your_google_gemini_api_key
```

### Run

**Desktop (PyQt6)**

```bash
python main_shell.py  # Full app shell (recommended)
```

**HTTP API (JSON backend for a separate frontend)**

```bash
python run_api.py     # http://127.0.0.1:8000 — see /docs for OpenAPI
```

On Windows, `run_api.bat` uses your project venv if present. The API serves JSON under `/api/…` and the bundled graph viewer under `/assets/graph/` (for iframe or dev-server proxy).

## App Shell

The shell (`gui/shell.py`) is a `QMainWindow` with a fixed 120px sidebar and a `QStackedWidget` that fills the remaining space. Pages and launchers are registered at startup in `gui/app_shell.py`:

```
AppShell
├── Sidebar (fixed, dark)
│   ├── Home        → HomePage      (stat cards, recent papers)
│   ├── Graph       → GraphPage     (D3 force graph + paper list)
│   ├── Projects    → ProjectsPage  (project list + detail view)
│   ├── Add by DOI  → DoiPage       (DOI resolution + save)
│   ├── Setup       → SetupPage     (API key instructions)
│   └── Search      → SearchWindow  (floating, not embedded)
└── QStackedWidget (page content)
```

New pages and launchers can be added in one line:

```python
shell.add_page("Stats", StatsWidget())        # embedded, switchable
shell.add_launcher("Settings", open_settings) # opens a floating window
```

`add_page` returns the stack index. `add_launcher` buttons are not checkable and do not affect the stack.

## Usage

### Projects

```python
from projects import Project, filter_projects, Q, Status

# Create and save a project
p = Project(name="Diffusion Models", color=0x5b8dee, project_tags=["generative"])
p.save()

# Add papers
p.add_paper("2006.11239")
p.add_papers(["2010.02502", "2112.10752"])

# Query with composable predicates
active = filter_projects(Q("status = ?", Status.ACTIVE))
not_deleted = filter_projects(~Q("status = ?", Status.DELETED))
blue_diffusion = filter_projects(
    Q("status = ?", Status.ACTIVE)
    & Q("color = ?", 0x5b8dee)
    & Q("name LIKE ?", "%diffusion%")
)
```

### Notes

```python
from notes import Note, get_notes, count_paper_notes, ensure_notes_db

ensure_notes_db()

# Add a project-scoped note on a paper
note = Note(paper_id="2006.11239", project_id=p.id, title="Key insight", content="...")
note.save()

# Retrieve
project_notes = get_notes("2006.11239", project_id=p.id)
count = count_paper_notes("2006.11239", project_id=p.id)
```

### Search and save papers

```python
from fetch_paper_metadata import search_papers, fetch_paper_metadata
from db import init_db

init_db()
papers = search_papers("lattice QCD", max_results=25)  # auto-saves to DB
```

### Add by DOI

Use the "Add by DOI" page in the app shell, or resolve programmatically:

```python
from gui.doi_page import _resolve_doi

result = _resolve_doi("10.48550/arXiv.1706.03762")
```

### AI tools

```python
from AI_tools import tag, summarize, find_related, PaperContent

content = PaperContent(abstract=paper.summary)

tags = tag(content)                        # ["#quantum_computing", ...]
tags = tag(content, file_path="tags.md")   # also appends to file

s = summarize(content)
print(s.tldr)
print(s.key_contributions)

# Semantic edges for the graph
from db import list_papers
candidates = [(r["paper_id"], r["summary"]) for r in list_papers()]
related_ids = find_related(content, candidates)
```

### Download PDFs

```python
from downloads import download_pdf, download_pdf_batch, download_source_batch

download_pdf(paper, dirpath="pdfs/")
download_pdf_batch(papers, dirpath="pdfs/")
download_source_batch(papers, dirpath="source/")
```

### Database queries

```python
from db import get_paper, get_all_versions, list_papers, get_graph_data

get_paper("2204.12985")           # latest version
get_paper("2204.12985", version=2)
get_all_versions("2204.12985")    # all stored versions
nodes, edges = get_graph_data()   # for the graph viewer
```

## Graph Visualization

Papers (blue circles) and authors (gold diamonds) form a force-directed network. Edges connect each paper to its authors. The control panel has four real-time sliders:

| Slider | Effect |
|---|---|
| Center force | Pulls/pushes nodes toward the center |
| Repel force | Controls node-to-node repulsion |
| Link force | Stiffness of paper–author edges |
| Link distance | Target edge length |

## Notes

- arXiv requests are rate-limited to one every 3 seconds per arXiv's API policy.
- `papers.db`, `pdfs/`, `source/`, and vault contents are gitignored.
- KaTeX, D3, and all fonts are bundled locally — the GUI works fully offline after first run.
- `PaperContent` accepts `abstract`, `full_text` (TeX source), or `pdf` (bytes) — Gemini will use the richest available source.

## Acknowledgements

linXiv owes a debt to [Qiqqa](https://github.com/jimmejardine/qiqqa-open-source), the open-source research management tool originally created by Jimme Jardine. Exploring the Qiqqa codebase (via a [personal fork](https://github.com/jakeuribe/qiqqa-open-source)) informed several design decisions in linXiv, particularly around library-oriented paper management, project organization, and the general approach of combining PDF handling with metadata storage in a desktop application.

Qiqqa is licensed under the [GNU General Public License v3.0](https://www.gnu.org/licenses/gpl-3.0.html).

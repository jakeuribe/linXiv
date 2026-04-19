# linXiv

A Python application for discovering, managing, and visualizing academic papers from arXiv. Combines a local SQLite database, OPTIONAL AI-powered tagging, Obsidian vault integration, and an interactive D3.js network graph, wrapped in a PyQt6 GUI.

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
├── AI_tools.py                # Gemini: tag(), summarize(), find_related(); PaperContent input type
├── api/
│   ├── __main__.py            # Entry point: python -m api
│   ├── app.py                 # FastAPI routes + /assets/graph (bundled D3 graph for iframe/proxy)
│   ├── graph_payload.py       # Graph JSON (tags + projects) for /api/graph
│   └── run_api.py             # uvicorn launcher helper
├── sources/
│   ├── base.py                # PaperSource ABC + PaperMetadata dataclass
│   ├── arxiv_source.py        # ArxivSource: search and fetch from arXiv API
│   ├── openalex_source.py     # OpenAlexSource: lookup via OpenAlex
│   ├── doi_resolve.py         # DOI resolution (arXiv, OpenAlex, CrossRef fallback)
│   ├── fetch_paper_metadata.py# High-level fetch/search helpers + Obsidian note generation
│   └── arxiv_downloads.py     # PDF and TeX source download helpers
├── storage/
│   ├── db.py                  # SQLite DB: versioned paper storage, graph data queries
│   ├── projects.py            # Projects: data model, Status enum, Q query builder
│   └── notes.py               # Notes: per-paper annotations scoped to projects
├── formats/
│   └── table_format.md        # YAML frontmatter template for Obsidian notes
├── gui/
│   ├── app_shell.py           # QApplication + AppShell wiring (run via main_shell.py)
│   ├── shell.py               # AppShell: sidebar nav + QStackedWidget page container
│   ├── home_page.py           # Home: stat cards, recent papers list
│   ├── graph_page.py          # Graph page (embedded in shell)
│   ├── library_page.py        # Library: full paper list with filtering
│   ├── projects_page.py       # Projects: list, detail view, add paper/note dialogs
│   ├── doi_page.py            # Add by DOI: three-strategy resolution + save to library
│   ├── setup_page.py          # Setup: API key instructions and status
│   ├── graph_view.py          # QWebEngineView wrapper for D3 graph
│   ├── markdown_view.py       # QWebEngineView wrapper for markdown rendering
│   ├── tex_view.py            # QWebEngineView wrapper for KaTeX rendering
│   ├── pdf_window.py          # QPdfView PDF viewer with toolbar
│   ├── search/
│   │   ├── _window.py         # Floating search UI: tri-pane with TeX rendering and PDF button
│   │   ├── _widgets.py        # Reusable search widget components
│   │   └── _workers.py        # QThread workers for async search
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
uv sync   # recommended (uses uv)
# or
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
python -m api         # http://127.0.0.1:8000 — see /docs for OpenAPI
```

The API serves JSON under `/api/…` and the bundled graph viewer under `/assets/graph/` (for iframe or dev-server proxy).

**CLI**

Install once (editable install via uv):

```bash
uv pip install -e .
```

Then run from anywhere:

```bash
linxiv search "attention is all you need" --max 5
linxiv fetch 2204.12985
linxiv list --limit 20 --category cs.LG
linxiv tag 2204.12985
linxiv project list
linxiv project create "Diffusion Models" --description "Score-based generative models"
linxiv project add-paper 1 2006.11239
linxiv note create 2204.12985 "Key insight: scaled dot-product attention" --title "Reading notes"
```

All commands output JSON (or a formatted markdown card for `fetch`). Pass `--help` to any subcommand for full options.

**MCP server (Claude integration)**

To expose linXiv as tools that Claude can call directly, add the `mcp` optional dependency and create a thin server:

```bash
uv pip install -e ".[mcp]"   # installs the mcp package (not included by default)
```

Then register it with Claude Code — the absolute path and `cwd` are required because `linxiv_mcp.py` uses relative imports:

```bash
claude mcp add linxiv -- uv run /absolute/path/to/linxiv/linxiv_mcp.py
```

Or add it manually to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "linxiv": {
      "command": "uv",
      "args": ["run", "linxiv_mcp.py"],
      "cwd": "/absolute/path/to/linxiv"
    }
  }
}
```

Once registered, Claude can call these tools directly in conversation: `search_papers`, `fetch_paper`, `list_papers`, `get_paper`, `search_full_text`, `tag_paper`, `list_projects`, `create_project`, `add_paper_to_project`, `remove_paper_from_project`, `create_note`, `get_notes_for_paper`, `get_notes_for_project`.

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
from storage import Project, filter_projects, Q, Status

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
from storage import Note, get_notes, count_paper_notes, ensure_notes_db

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
from sources import search_papers, fetch_paper_metadata
from storage import init_db

init_db()
papers = search_papers("lattice QCD", max_results=25)  # auto-saves to DB
```

### Add by DOI

Use the "Add by DOI" page in the app shell, or resolve programmatically:

```python
from sources import resolve_doi

result = resolve_doi("10.48550/arXiv.1706.03762")
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
from storage import list_papers
candidates = [(r["paper_id"], r["summary"]) for r in list_papers()]
related_ids = find_related(content, candidates)
```

### Download PDFs

```python
from sources.arxiv_downloads import download_pdf, download_pdf_batch, download_source_batch

download_pdf(paper, dirpath="pdfs/")
download_pdf_batch(papers, dirpath="pdfs/")
download_source_batch(papers, dirpath="source/")
```

### Database queries

```python
from storage import get_paper, get_all_versions, list_papers, get_graph_data

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

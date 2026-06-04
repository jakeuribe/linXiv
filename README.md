# linXiv

<img src="assets/wide_logo.png" alt="linXiv logo" width="800" />

A local-first desktop application for discovering, managing, and visualizing academic papers from arXiv and other sources. Combines a local SQLite database, optional AI-powered tagging, Obsidian vault integration, and an interactive D3.js network graph, wrapped in a Tauri desktop shell (React + TypeScript frontend, Python backend).

Upload your PDFs, create projects, manage notes, tags, and more to organize your files — all locally, without sending your data anywhere. This project aims to be a one-stop-shop for researchers who want to manage their literature, with the near-term goal of extending to research groups who seek to share knowledge without going to the web.

> **Development status:** The database schema and paper identifier format are actively changing. `source_id` values are being migrated to a namespaced format (`arxiv:2204.12985`, `doi:10.48550/…`, `openalex:W3123456789`, `local:{hash}`). Until that work lands, pre-v0.1.1 (current version) existing `papers.db` files will not be compatible with new builds — delete `papers.db` and let it rebuild on first run. No stable release has been cut yet. Migrations from version 0.1.0 to 0.1.1 will be accounted for.


<img src="assets/carousel.gif" width="800" />

## Table of Contents

- [Features](#features)
- [Project Structure](#project-structure)
- [Setup](#setup)
  - [Prerequisites](#prerequisites)
  - [Install dependencies](#install-dependencies)
  - [Environment variables](#environment-variables)
  - [Run](#run)
- [Building the Tauri App](#building-the-tauri-app)
  - [Tauri prerequisites](#tauri-prerequisites)
  - [Development](#development)
  - [Production build](#production-build)
- [Usage](#usage)
  - [Projects](#projects)
  - [Notes](#notes)
  - [Search and save papers](#search-and-save-papers)
  - [Add by DOI](#add-by-doi)
  - [AI tools](#ai-tools)
  - [Download PDFs](#download-pdfs)
  - [Database queries](#database-queries)
- [Graph Visualization](#graph-visualization)
- [Acknowledgements](#acknowledgements)

## Features

- **Paper search** — Search arXiv by keyword, fetch by ID, or look up by DOI; results saved to a local SQLite DB with version tracking
- **Interactive graph** — Force-directed D3.js visualization of papers and authors; real-time force controls (gravity, repulsion, link strength)
- **Projects** — Organise papers into projects; add notes per paper scoped to a project; composable SQL query builder (`Q`) for filtering
- **TeX rendering** — KaTeX renders LaTeX math in titles and abstracts inside the search UI
- **AI tools** — Google Gemini structured output for tag generation, paper summarization, and semantic similarity
- **Obsidian integration** — Auto-generate markdown notes with YAML frontmatter for your vault
- **PDF & TeX downloads** — Batch download PDFs and TeX source tarballs

## Project Structure

```
linXiv/
├── AI_tools.py                # Gemini: tag(), summarize(), find_related(); PaperContent input type
├── linxiv_cli.py              # CLI entry point (linxiv command via pyproject.toml)
├── linxiv_mcp.py              # MCP server for Claude integration
├── config.py                  # App-wide configuration constants
├── user_settings.py           # User-editable settings (API keys, paths)
├── pyproject.toml             # Package metadata + CLI/MCP entry points
├── assets/
│   ├── app_icon.png           # Application icon
│   └── wide_logo.png          # Wide logo (README header)
├── api/
│   ├── __main__.py            # Entry point: python -m api
│   ├── app.py                 # FastAPI routes + /assets/graph (bundled graph for iframe/proxy)
│   ├── graph_payload.py       # Graph JSON (tags + projects) for /api/graph
│   └── run_api.py             # uvicorn launcher helper
├── sources/
│   ├── base.py                # PaperSource protocol + PaperMetadata model
│   ├── arxiv_source.py        # ArxivSource: search and fetch from arXiv API
│   ├── crossref_source.py     # CrossRefSource: fetch by DOI, search by title
│   ├── openalex_source.py     # OpenAlexSource: lookup via OpenAlex
│   ├── doi_resolve.py         # DOI resolution (arXiv, Semantic Scholar, CrossRef fallback)
│   ├── fetch_paper_metadata.py# High-level fetch/search helpers + Obsidian note generation
│   ├── pdf_metadata.py        # PDF metadata extraction and resolution pipeline
│   └── arxiv_downloads.py     # PDF and TeX source download helpers
├── service/
│   ├── paper.py               # Paper service: get, get_all, get_many, upsert, graph data
│   ├── author.py              # Author service: get, upsert, link/unlink to papers
│   ├── tag.py                 # Tag service: get, upsert, paper/project tag management
│   ├── note.py                # Note service: get, upsert, count by paper/project
│   ├── project.py             # Project service: get, upsert, filter, status management
│   ├── content.py             # Content service: full-text and file content
│   ├── files.py               # File utilities for paper sources
│   └── models/                # Typed return types (PaperDetails, ProjectDetails, etc.)
├── storage/
│   ├── db.py                  # SQLite DB: versioned paper storage, graph data queries
│   ├── authors.py             # Author CRUD and paper linkage
│   ├── tags.py                # Tag CRUD
│   ├── projects.py            # Projects: data model, Status enum, Q query builder
│   ├── notes.py               # Notes: per-paper annotations scoped to projects
│   ├── paths.py               # Filesystem paths (project root, DB, PDFs)
│   ├── config/
│   │   ├── core.py            # Schema application: apply_sql_schema, init_db
│   │   ├── queries.py         # Typed query helpers + composable Q predicate builder
│   │   └── sql/               # SQL table, view, and index definitions
│   └── migrations/            # One-off schema migration scripts
├── formats/
│   ├── table_format.md        # YAML frontmatter template for Obsidian notes
│   └── arxiv_paper.md         # Plain-text paper card template
├── public/
│   └── graph/                 # Graph viewer (graph.js + graph.css) served by the API
├── src/                       # React + TypeScript frontend (Vite)
├── src-tauri/                 # Tauri shell (Rust) + bundled sidecar binaries
├── tests/                     # pytest suite (API, CLI, DB, sources, DOI, notes, projects)
├── docs/                      # Development notes and technical debt log
└── pdfs/                      # Downloaded PDFs (gitignored)
```

## Setup

### Prerequisites

- Python 3.10+
- [Node.js](https://nodejs.org/) 18+ (for frontend / Tauri dev)
- [Rust toolchain](https://rustup.rs/) (for Tauri)
- [uv](https://github.com/astral-sh/uv) (recommended Python package manager)

### Install dependencies

```bash
uv sync          # Python dependencies (backend + dev)
npm install      # Node dependencies (frontend)
```

> Add `--extra mcp` if you need the MCP server: `uv pip install -e ".[mcp]"`

### Environment variables

Create a `.env` file in the project root:

```env
GENAI_API_KEY_TAG_GEN=your_google_gemini_api_key
```

### Run

**HTTP API (JSON backend)**

```bash
uv run python -m api   # http://127.0.0.1:8000 — see /docs for OpenAPI
```

**CLI**

Install once (editable install via uv):

```bash
uv pip install -e .
```

Then run from anywhere:

```bash
linxiv --version

# Search papers (arxiv, openalex, or crossref)
linxiv search "attention is all you need" --max 5
linxiv search "diffusion models" --source openalex --max 10
linxiv search "lattice QCD" --source crossref --max 3

# Fetch and save a paper by ID
linxiv fetch 2204.12985
linxiv fetch W3123456789 --source openalex

# List papers in the database
linxiv list --limit 20 --offset 0 --category cs.LG

# Paper management
linxiv paper get 2204.12985
linxiv paper versions 2204.12985
linxiv paper delete 2204.12985

# Tag management
linxiv tag add 2204.12985 transformers attention deep-learning
linxiv tag remove 2204.12985 attention
linxiv tag list 2204.12985
linxiv tag list-all
linxiv tag create my-tag
linxiv tag delete 42

# Project management
linxiv project list
linxiv project list --status active      # active | archived | deleted
linxiv project get 1
linxiv project create "Diffusion Models" --description "Score-based generative models"
linxiv project update 1 --name "Diffusion Models v2" --description "Updated"
linxiv project add-paper 1 2006.11239
linxiv project remove-paper 1 2006.11239
linxiv project delete 1

# Note management
linxiv note create 2204.12985 "Key insight: scaled dot-product attention" --title "Reading notes"
linxiv note create 2204.12985 "Follow-up question" --project-id 1
linxiv note get 7
linxiv note list --paper-id 2204.12985
linxiv note list --project-id 1
linxiv note delete 7

# PDF management
linxiv pdf path 2204.12985
linxiv pdf path 2204.12985 --version 2
linxiv pdf download 2204.12985 https://arxiv.org/pdf/2204.12985
linxiv pdf storage
```

All commands output JSON (or a formatted markdown card for `fetch`). Pass `--help` to any subcommand for full options.

**MCP server (Claude integration)**

Install with the `mcp` extra:

```bash
uv pip install -e ".[mcp]"
```

Register with Claude Code:

```bash
claude mcp add linxiv -- linxiv-mcp
```

Or add manually to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "linxiv": {
      "command": "linxiv-mcp"
    }
  }
}
```

> Without an editable install, fall back to `uv run`:
> ```json
> { "command": "uv", "args": ["run", "linxiv_mcp.py"], "cwd": "/absolute/path/to/linxiv" }
> ```

Once registered, Claude can call these tools directly: `search_papers`, `fetch_paper`, `list_papers`, `get_paper`, `search_full_text`, `tag_paper`, `list_projects`, `create_project`, `add_paper_to_project`, `remove_paper_from_project`, `create_note`, `get_notes_for_paper`, `get_notes_for_project`.

## Building the Tauri App

The Tauri desktop app wraps the React/Vite frontend and bundles the Python backend as sidecar binaries compiled with PyInstaller.

### Tauri prerequisites

- [Node.js](https://nodejs.org/) 18+
- [Rust toolchain](https://rustup.rs/) (stable)
- [uv](https://github.com/astral-sh/uv)
- System Tauri dependencies — follow the [Tauri v2 prerequisites guide](https://tauri.app/start/prerequisites/) for your OS (WebKit2GTK on Linux, Xcode Command Line Tools on macOS, Microsoft C++ Build Tools on Windows)

### Development

Start the Python API and the Tauri dev window in separate terminals:

```bash
# terminal 1 — Python backend
uv run python -m api   # http://127.0.0.1:8000

# terminal 2 — Tauri dev window (also starts Vite, hot-reloads on frontend changes)
npm run tauri dev
```

The Python API sidecar is not bundled in dev mode — the app talks to the locally running API on port 8000.

### Production build

The Python entry points (API, CLI, MCP server) are compiled to self-contained binaries with PyInstaller and staged into `src-tauri/binaries/` before Tauri bundles the app.

**1. Build and stage the Python sidecars:**

```bash
npm run build:sidecar
```

This runs PyInstaller on `linxiv-api.spec`, `linxiv-cli.spec`, and `linxiv-mcp.spec`, then copies the outputs to `src-tauri/binaries/` with the correct Tauri target-triple suffix.

**2. Build the Tauri app:**

```bash
npm run tauri build
```

Or run both steps at once:

```bash
npm run build:all
```

The final installer/bundle is written to `src-tauri/target/release/bundle/`.

**Installing the CLI**

After installing the desktop app, open Settings and click **Install CLI** to symlink the bundled `linxiv` binary to `~/.local/bin/linxiv` (Linux/macOS) or add a shim to your PATH (Windows).

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
- KaTeX, D3, and all fonts are bundled locally — the app works fully offline after first run.
- `PaperContent` accepts `abstract`, `full_text` (TeX source), or `pdf` (bytes) — Gemini will use the richest available source.

## Acknowledgements

linXiv owes a debt to [Qiqqa](https://github.com/jimmejardine/qiqqa-open-source), the open-source research management tool originally created by Jimme Jardine. Exploring the Qiqqa codebase (via a [personal fork](https://github.com/jakeuribe/qiqqa-open-source)) informed several design decisions in linXiv, particularly around library-oriented paper management, project organization, and the general approach of combining PDF handling with metadata storage in a desktop application.

Qiqqa is licensed under the [GNU General Public License v3.0](https://www.gnu.org/licenses/gpl-3.0.html).

PDF text and metadata extraction uses [pypdf](https://github.com/py-pdf/pypdf), a pure-Python PDF library maintained by the [py-pdf](https://github.com/py-pdf) organization. pypdf is licensed under the [BSD 3-Clause License](https://github.com/py-pdf/pypdf/blob/main/LICENSE).

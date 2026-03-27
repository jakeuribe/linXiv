# linXiv

A Python application for discovering, managing, and visualizing academic papers from arXiv. Combines a local SQLite database, AI-powered tagging, Obsidian vault integration, and an interactive D3.js network graph — all wrapped in a PyQt6 GUI.

## Features

- **Paper search** — Search arXiv by keyword or fetch by ID; results saved to a local SQLite DB with version tracking
- **Interactive graph** — Force-directed D3.js visualization of papers and authors; real-time force controls (gravity, repulsion, link strength)
- **TeX rendering** — KaTeX renders LaTeX math in titles and abstracts inside the search UI
- **PDF viewer** — Native Qt PDF viewer (`QPdfView`) with zoom and page navigation
- **AI tools** — Google Gemini structured output for tag generation, paper summarization, and semantic similarity
- **Obsidian integration** — Auto-generate markdown notes with YAML frontmatter for your vault
- **PDF & TeX downloads** — Batch download PDFs and TeX source tarballs

## Project Structure

```
linXiv/
├── main.py                    # Launch graph viewer
├── search.py                  # Launch search window
├── pdf.py                     # Launch standalone PDF viewer
├── db.py                      # SQLite DB: versioned paper storage, graph data queries
├── fetch_paper_metadata.py    # arXiv API: fetch, search, save, generate Obsidian notes
├── downloads.py               # PDF and TeX source download helpers
├── AI_tools.py                # Gemini: tag(), summarize(), find_related(); PaperContent input type
├── formats/
│   └── table_format.md        # YAML frontmatter template for Obsidian notes
├── gui/
│   ├── app.py                 # QApplication setup
│   ├── main_window.py         # Graph window
│   ├── search_window.py       # Search UI: tri-pane layout with TeX rendering and PDF button
│   ├── graph_view.py          # QWebEngineView wrapper for D3 graph
│   ├── tex_view.py            # QWebEngineView wrapper for KaTeX TeX rendering
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
pip install arxiv PyQt6 PyQt6-WebEngine google-genai pydantic python-dotenv
```

### Environment variables

Create a `.env` file in the project root:

```env
GENAI_API_KEY_TAG_GEN=your_google_gemini_api_key
```

### Run

```bash
python main.py    # Graph viewer
python search.py  # Search window
python pdf.py     # PDF viewer (opens file picker)
```

## Usage

### Search and save papers

```python
from fetch_paper_metadata import search_papers, fetch_paper_metadata
from db import init_db

init_db()
papers = search_papers("lattice QCD", max_results=25)  # auto-saves to DB
```

### Generate Obsidian notes

```python
from fetch_paper_metadata import gen_md_files

gen_md_files(papers, additional_tags=["lattice_qcd"])
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

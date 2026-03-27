# linXiv

A Python application for discovering, managing, and visualizing academic papers from arXiv. Combines a local SQLite database, AI-powered tagging, Obsidian vault integration, and an interactive D3.js network graph — all wrapped in a PyQt6 GUI.

## Features

- **Paper discovery** — Fetch paper metadata from arXiv by ID or keyword search
- **Local storage** — SQLite database with versioning support
- **PDF & source downloads** — Batch download PDFs and TeX source tarballs
- **Obsidian integration** — Auto-generate markdown notes with YAML frontmatter for your vault
- **AI tagging** — Use Google Gemini to generate Obsidian-formatted tags from abstracts
- **Interactive graph** — Force-directed D3.js visualization of papers and co-authorship networks, rendered in a PyQt6 WebEngine window

## Project Structure

```
linXiv/
├── main.py                    # Entry point — launches the GUI
├── db.py                      # SQLite database management
├── fetch_paper_metadata.py    # arXiv API fetching + Obsidian markdown generation
├── downloads.py               # PDF and TeX source download utilities
├── AI_tools.py                # Google Gemini tag generation
├── basic_connect.py           # PostgreSQL connection test (legacy)
├── print_markdown.py          # Markdown template rendering utility
├── formats/
│   └── table_format.md        # YAML frontmatter template for Obsidian notes
├── gui/
│   ├── app.py                 # PyQt6 application factory
│   ├── main_window.py         # Main window
│   ├── graph_view.py          # WebEngine wrapper for D3 visualization
│   └── web/
│       ├── graph.html         # SVG container + force simulation controls
│       ├── graph.js           # D3.js force-directed graph logic
│       ├── graph.css          # Dark theme styling
│       └── d3.v7.min.js       # Bundled D3 library
└── obsidian_vault/
    └── arXivVault/            # Generated markdown notes (gitignored)
```

## Setup

### Prerequisites

- Python 3.10+
- PyQt6 with WebEngine support

### Install dependencies

```bash
pip install arxiv PyQt6 PyQt6-WebEngine google-generativeai python-dotenv
```

### Environment variables

Create a `.env` file in the project root:

```env
GENAI_API_KEY_TAG_GEN=your_google_gemini_api_key
```

### Run

```bash
python main.py
```

## Usage

### Fetch a paper by arXiv ID

```python
from fetch_paper_metadata import fetch_paper_metadata
from db import init_db, save_paper

conn = init_db()
paper = fetch_paper_metadata("2204.12985")
save_paper(conn, paper)
```

### Search papers by keyword

```python
from fetch_paper_metadata import search_papers
from db import save_papers

results = search_papers("transformer attention mechanism", max_results=10)
save_papers(conn, results)
```

### Generate Obsidian notes

```python
from fetch_paper_metadata import gen_md_files

gen_md_files(papers, vault_path="obsidian_vault/arXivVault")
```

### Generate AI tags for a note

```python
from AI_tools import generate_tags

generate_tags("obsidian_vault/arXivVault/2204.12985v1.md")
```

### Download PDFs

```python
from downloads import download_pdf_batch

download_pdf_batch(papers, output_dir="pdfs/")
```

## Graph Visualization

The GUI displays papers (blue circles) and authors (orange diamonds) as a force-directed network. Nodes are connected when an author contributed to a paper. Use the control panel sliders to tune the simulation forces in real time.

## Notes

- arXiv requests are rate-limited to one every 3 seconds per arXiv's usage policy.
- The SQLite database (`papers.db`) and vault contents are gitignored.
- PostgreSQL support in `basic_connect.py` is a legacy remnant and not used by the main application.

# CLI Reference

The `linxiv` binary is a headless interface to the same library the app uses. In a checkout you can run it without installing:

```bash
# from src-tauri/
cargo run -p linxiv-cli -- --help
```

Installed (via the app's **Install CLI**, or a staged/bundled build), invoke it directly as `linxiv`. All commands print JSON to stdout; pass `--help` to any command or subcommand for full options.

```bash
linxiv --version

# Search (source: arxiv (default), openalex, or crossref)
linxiv search "attention is all you need" --max 5
linxiv search "diffusion models" --source openalex --max 10
linxiv search "lattice QCD" --source crossref --max 3

# Fetch and save a paper by ID
linxiv fetch 2204.12985
linxiv fetch W3123456789 --source openalex

# List stored papers
linxiv list --limit 20 --offset 0 --category cs.LG

# Papers
linxiv paper get 2204.12985
linxiv paper versions 2204.12985
linxiv paper search "scaled dot-product"     # full-text search of the local library
linxiv paper fetch-source 2204.12985         # pull the arXiv TeX source so `search` can find it
linxiv paper fetch-source 2204.12985 --force # re-fetch a paper already indexed
linxiv paper index-sources --limit 25        # backfill papers with no TeX source yet (~7s each)
linxiv paper doi-candidates 2204.12985       # other paper roots sharing this paper's DOI
linxiv paper repair 2204.12985 --title "Attention Is All You Need" --authors "A. Vaswani" "N. Shazeer" --published 2017-06-12 --summary "..." --category cs.CL --doi 10.48550/arXiv.1706.03762 --url https://arxiv.org/abs/1706.03762 --tags attention transformers
linxiv paper delete 2204.12985               # soft-delete
linxiv paper restore 2204.12985
linxiv paper hard-delete 2204.12985
linxiv paper remove-from-all-projects 2204.12985

# Tags (on papers)
linxiv tag add 2204.12985 transformers attention deep-learning
linxiv tag remove 2204.12985 attention
linxiv tag list 2204.12985
linxiv tag list-all
linxiv tag create my-tag
linxiv tag delete 42
# Tags (on projects)
linxiv tag add-project 1 reading-list
linxiv tag remove-project 1 reading-list
linxiv tag list-project 1

# Projects
linxiv project list
linxiv project list --status active                # active | archived | deleted
linxiv project get 1
linxiv project create "Diffusion Models" --description "Score-based generative models" --color "#4f86f7" --tags generative
linxiv project update 1 --name "Diffusion Models v2" --status archived
linxiv project add-paper 1 2006.11239
linxiv project add-papers 1 2006.11239 2204.12985  # bulk; reports added + failed ids
linxiv project remove-paper 1 2006.11239
linxiv project archive 1
linxiv project restore 1
linxiv project delete 1                            # soft-delete
linxiv project hard-delete 1
linxiv project export 1 ./diffusion --pdfs         # .lxproj archive
linxiv project import ./diffusion.lxproj --on-conflict merge   # merge | overwrite; --preview for a dry run
linxiv project export-bibtex 1 ./diffusion.bib
linxiv project export-obsidian 1 ./diffusion.md

# Notes
linxiv note create 2204.12985 "Key insight: scaled dot-product attention" --title "Reading notes"
linxiv note create 2204.12985 "Follow-up question" --project-id 1
linxiv note get 7
linxiv note list --paper-id 2204.12985
linxiv note list --project-id 1
linxiv note update 7 --content "Revised note"
linxiv note delete 7

# PDF highlight annotations
linxiv annotation create 2204.12985 '<anchor-json>' --comment "important" --project-id 1
linxiv annotation list --paper-id 2204.12985
linxiv annotation get 3
linxiv annotation update 3 --comment "revised"
linxiv annotation delete 3

# PDFs
linxiv pdf path 2204.12985
linxiv pdf path 2204.12985 --version 2
linxiv pdf download 2204.12985 https://arxiv.org/pdf/2204.12985
linxiv pdf download 2204.12985 https://arxiv.org/pdf/2204.12985v2 --version 2
linxiv pdf import ./local-paper.pdf --project-id 1
linxiv pdf list                                  # papers with a PDF saved on disk, largest first
linxiv pdf delete 2204.12985                     # remove every saved version's PDF, keep the paper
linxiv pdf storage

# DOI
linxiv doi resolve 10.48550/arXiv.1706.03762     # resolve to metadata, no save
linxiv doi save 10.48550/arXiv.1706.03762        # resolve and save to library

# Authors
linxiv author list
linxiv author get 12
linxiv author update 12 --full-name "A. N. Other"
linxiv author delete 12                          # blocked if still linked to papers
linxiv author merge-candidates 12                # other authors sharing author 12's ORCID
linxiv author merge 12 34 56                     # fold 34 and 56 into canonical author 12

# BibTeX import
linxiv bibtex import ./refs.bib
linxiv bibtex import ./refs.bib --project-id 1   # link every imported paper to a project

# Trash (soft-deleted items)
linxiv trash list
linxiv trash restore 2204.12985
linxiv trash hard-delete 2204.12985
linxiv trash restore-project 1
linxiv trash hard-delete-project 1

# Library / maintenance
linxiv stats
linxiv categories
linxiv settings get
linxiv settings update <key> <value>             # value is JSON-parsed if valid JSON, else a string
linxiv backup ./papers.bak                       # snapshot the DB
linxiv restore ./papers.bak                      # restore even if the live DB is broken
```
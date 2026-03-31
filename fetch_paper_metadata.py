import os
import time
import arxiv
from datetime import datetime
from pathlib import Path
from typing import Sequence, Generator, Iterable, Iterator
from db import init_db, save_paper, save_papers

# Shared client — 5 s gap gives margin over arXiv's 3 s minimum
_client = arxiv.Client(num_retries=3, delay_seconds=7.0)

_RATELIMIT_FILE = str(Path(__file__).parent / ".arxiv_ratelimit")
_VAULT_DIR      = Path(__file__).parent / "obsidian_vault" / "arXivVault"
_RATELIMIT_WAIT = 60.0  # seconds to wait after a 429


def _check_ratelimit() -> None:
    """If a 429 was recently recorded, sleep for the remaining cooldown."""
    if not os.path.exists(_RATELIMIT_FILE):
        return
    with open(_RATELIMIT_FILE) as f:
        last = datetime.fromisoformat(f.read().strip())
    remaining = _RATELIMIT_WAIT - (datetime.now() - last).total_seconds()
    if remaining > 0:
        print(f"[arxiv] rate limited — waiting {remaining:.0f}s")
        time.sleep(remaining)


def _record_ratelimit() -> None:
    with open(_RATELIMIT_FILE, "w") as f:
        f.write(datetime.now().isoformat())


def _arxiv_call(fn):
    _check_ratelimit()
    try:
        return fn()
    except Exception as e:
        if "429" in str(e):
            _record_ratelimit()
            print("[arxiv] 429 received — recorded. Retry your search in 60s.")
        raise

def fetch_paper_metadata(paper_id:str, print_on: bool = False) -> arxiv.Result:
    search = arxiv.Search(id_list=[paper_id])
    paper = _arxiv_call(lambda: next(_client.results(search)))
    
    if print_on:
        # Display Metadata
        print(f"Title:    {paper.title}")
        print(f"Date:     {paper.published.strftime('%Y-%m-%d')}")
        print(f"Authors:  {', '.join(author.name for author in paper.authors)}")
        print(f"Category: {paper.primary_category}")
        print(f"DOI:      {paper.doi}")
        print(f"PDF URL:  {paper.pdf_url}")
        print("-" * 30)
        print(f"Abstract:\n{paper.summary}")
    
    return paper

def search_papers(
    query: str,
    max_results: int = 10,
    sort_by: arxiv.SortCriterion = arxiv.SortCriterion.Relevance,
    sort_order: arxiv.SortOrder = arxiv.SortOrder.Descending,
    print_on: bool = False,
) -> list[arxiv.Result]:
    search = arxiv.Search(query=query, max_results=max_results, sort_by=sort_by, sort_order=sort_order)
    papers = _arxiv_call(lambda: list(_client.results(search)))

    if print_on:
        for paper in papers:
            print(f"Title:    {paper.title}")
            print(f"Date:     {paper.published.strftime('%Y-%m-%d')}")
            print(f"Authors:  {', '.join(author.name for author in paper.authors)}")
            print(f"Category: {paper.primary_category}")
            print("-" * 30)

    return papers

def gen_md_files(papers: list[arxiv.Result], additional_tags:None|Sequence[str] = None) -> None:
    
    for paper in papers:
        gen_md_file(paper, additional_tags=additional_tags)

def gen_md_file(paper: arxiv.Result, additional_tags:None|Sequence[str] = None, print_on: bool = False):
    # Access the argument using args.filename
    title:str = paper.title
    paper_id:str = paper.entry_id.split('/')[-1]
    url:str = f"https://arxiv.org/abs/{paper_id}"
    authors:list[str] = [author.name for author in paper.authors]
    tags:list[str] = ["clippings", "research", "clipping"]

    if additional_tags is not None:
        for s in additional_tags:
            tags.append(s)

    date = paper.published.strftime('%Y-%m-%d')
    filename = _VAULT_DIR / f"{paper_id}.md"

    author_list = "\n".join([f'  - "[[{name}]]"' for name in authors])
    tag_list = "\n".join([f'- {tag}' for tag in tags])
    with open(Path(__file__).parent / "formats" / "table_format.md", "r", encoding="utf-8") as f:
        template = f.read()
        final_content = template.format(
            title=title,
            url=url,
            author_list=author_list,
            date=date,
            tag_list=tag_list
        )
    if print_on:
        print(final_content)
    with open(filename, "w", encoding="utf-8") as f:
        f.write(final_content)

# Dev scaffold removed — use the app shell or call functions directly from a REPL.
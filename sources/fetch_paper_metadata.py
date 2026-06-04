import time
import arxiv
from datetime import datetime
from pathlib import Path
from typing import Sequence

from config import data_dir, resources_dir

_client = arxiv.Client(num_retries=1, delay_seconds=7.0)

_RATELIMIT_WAIT = 60.0


def _vault_dir() -> Path:
    """Obsidian vault location — resolved off data_dir() so it tracks LINXIV_DATA_DIR
    like the DB and PDFs, instead of being pinned to the source tree. Called per-use
    (not cached at import) to avoid the import-time-frozen trap."""
    return data_dir() / "obsidian_vault" / "arXivVault"


def _ratelimit_file() -> Path:
    """arXiv rate-limit timestamp file — lives in data_dir() so it tracks LINXIV_DATA_DIR
    like the DB, PDFs, and vault. Resolved per-use (not cached at import)."""
    return data_dir() / ".arxiv_ratelimit"


def _check_ratelimit() -> None:
    path = _ratelimit_file()
    if not path.exists():
        return
    with open(path) as f:
        last = datetime.fromisoformat(f.read().strip())
    remaining = _RATELIMIT_WAIT - (datetime.now() - last).total_seconds()
    if remaining > 0:
        print(f"[arxiv] rate limited — waiting {remaining:.0f}s")
        time.sleep(remaining)


def _record_ratelimit() -> None:
    path = _ratelimit_file()
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w") as f:
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

def fetch_paper_metadata(source_id: str, print_on: bool = False) -> arxiv.Result:
    search = arxiv.Search(id_list=[source_id])
    paper = _arxiv_call(lambda: next(_client.results(search)))

    if print_on:
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

def gen_md_files(papers: list[arxiv.Result], additional_tags: None | Sequence[str] = None) -> None:
    for paper in papers:
        gen_md_file(paper, additional_tags=additional_tags)

def gen_md_file(paper: arxiv.Result, additional_tags: None | Sequence[str] = None, print_on: bool = False):
    title: str = paper.title
    bare_id: str = paper.entry_id.split('/')[-1]
    source_id: str = f"arxiv:{bare_id}"
    url: str = f"https://arxiv.org/abs/{bare_id}"
    authors: list[str] = [author.name for author in paper.authors]
    tags: list[str] = ["clippings", "research", "clipping"]

    if additional_tags:
        for s in additional_tags:
            tags.append(s)

    date = paper.published.strftime('%Y-%m-%d')
    vault = _vault_dir()
    vault.mkdir(parents=True, exist_ok=True)
    filename = vault / f"{bare_id}.md"

    author_list = "\n".join([f'  - "[[{name}]]"' for name in authors])
    tag_list = "\n".join([f'- {tag}' for tag in tags])
    with open(resources_dir() / "formats" / "table_format.md", "r", encoding="utf-8") as f:
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

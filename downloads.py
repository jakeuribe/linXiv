import os
import re
import arxiv
from urllib.parse import urlparse
from urllib.request import urlretrieve

DOWNLOAD_DOMAIN = "export.arxiv.org"

def _substitute_domain(url: str, domain: str) -> str:
    parsed = urlparse(url)
    return parsed._replace(netloc=domain).geturl()

def _default_filename(paper: arxiv.Result, extension: str) -> str:
    raw = paper.entry_id.split('/')[-1]   # e.g. '2204.12985v4'
    safe = re.sub(r'[^\w.\-]', '_', raw)
    return f"{safe}.{extension}"

def download_pdf(
    paper: arxiv.Result,
    dirpath: str = './',
    filename: str = '',
    domain: str = DOWNLOAD_DOMAIN,
) -> str:
    """Download the PDF for a paper. Returns the path written to."""
    if paper.pdf_url is None:
        raise ValueError(f"No PDF URL for {paper.entry_id}")
    if not filename:
        filename = _default_filename(paper, "pdf")
    path = os.path.join(dirpath, filename)
    written, _ = urlretrieve(_substitute_domain(paper.pdf_url, domain), path)
    return written

def download_source(
    paper: arxiv.Result,
    dirpath: str = './',
    filename: str = '',
    domain: str = DOWNLOAD_DOMAIN,
) -> str:
    """Download the TeX source tarball for a paper. Returns the path written to."""
    if paper.pdf_url is None:
        raise ValueError(f"No source URL for {paper.entry_id}")
    source_url = paper.pdf_url.replace('/pdf/', '/src/')
    if not filename:
        filename = _default_filename(paper, "tar.gz")
    path = os.path.join(dirpath, filename)
    written, _ = urlretrieve(_substitute_domain(source_url, domain), path)
    return written

def download_pdf_batch(
    papers: list[arxiv.Result],
    dirpath: str = './',
    domain: str = DOWNLOAD_DOMAIN,
) -> list[str]:
    """Download PDFs for a list of papers. Returns list of paths written."""
    os.makedirs(dirpath, exist_ok=True)
    return [download_pdf(p, dirpath=dirpath, domain=domain) for p in papers]

def cleanup_pdfs(
    dirpath: str,
    keep: set[str] | None = None,
) -> list[str]:
    """Delete all PDFs in dirpath that are not in keep. Returns list of deleted paths."""
    if keep is None:
        keep = set()
    keep_abs = {os.path.abspath(p) for p in keep}
    deleted: list[str] = []
    for fname in os.listdir(dirpath):
        if not fname.lower().endswith(".pdf"):
            continue
        full = os.path.join(dirpath, fname)
        if os.path.abspath(full) not in keep_abs:
            os.remove(full)
            deleted.append(full)
    return deleted


def saved_pdfs_size(paths: set[str]) -> int:
    """Return total byte size of existing files in paths."""
    return sum(os.path.getsize(p) for p in paths if os.path.isfile(p))

def download_source_batch(
    papers: list[arxiv.Result],
    dirpath: str = './',
    domain: str = DOWNLOAD_DOMAIN,
) -> list[str]:
    """Download TeX source tarballs for a list of papers. Returns list of paths written."""
    os.makedirs(dirpath, exist_ok=True)
    return [download_source(p, dirpath=dirpath, domain=domain) for p in papers]

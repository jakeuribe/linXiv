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

def download_source_batch(
    papers: list[arxiv.Result],
    dirpath: str = './',
    domain: str = DOWNLOAD_DOMAIN,
) -> list[str]:
    """Download TeX source tarballs for a list of papers. Returns list of paths written."""
    os.makedirs(dirpath, exist_ok=True)
    return [download_source(p, dirpath=dirpath, domain=domain) for p in papers]

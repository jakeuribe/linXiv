"""Tests for sources/arxiv_downloads.py.

Covers pure helpers inline; network-dependent functions (download_pdf,
download_source, download_pdf_batch, download_source_batch) need urlretrieve
mocking or a local HTTP fixture — marked TODO below.
"""

from __future__ import annotations


# ---------------------------------------------------------------------------
# _substitute_domain
# ---------------------------------------------------------------------------

class TestSubstituteDomain:
    # TODO: test that https://arxiv.org/pdf/... → https://export.arxiv.org/pdf/...
    # TODO: test that scheme and path are preserved
    # TODO: test with query strings
    pass


# ---------------------------------------------------------------------------
# _default_filename
# ---------------------------------------------------------------------------

class TestDefaultFilename:
    # TODO: test that entry_id tail becomes the filename stem
    # TODO: test that unsafe characters are replaced with _
    # TODO: test extension is appended correctly ("pdf", "tar.gz")
    pass


# ---------------------------------------------------------------------------
# _strip_tex_noise
# ---------------------------------------------------------------------------

class TestStripTexNoise:
    # TODO: test that % comments are removed
    # TODO: test that escaped \% is preserved
    # TODO: test that \usepackage{...} is removed
    # TODO: test that \documentclass{...} is removed
    # TODO: test that \cite{...} is removed
    # TODO: test that normal content is preserved
    pass


# ---------------------------------------------------------------------------
# download_pdf / download_source (network — requires mocking urlretrieve)
# ---------------------------------------------------------------------------

class TestDownloadPdf:
    # TODO: mock urlretrieve and verify the correct URL is fetched
    # TODO: test that the domain substitution is applied
    # TODO: test that a custom filename is used when provided
    # TODO: test ValueError when paper.pdf_url is None
    pass


class TestDownloadSource:
    # TODO: mock urlretrieve; verify /pdf/ → /src/ substitution in URL
    # TODO: test that domain substitution is applied
    # TODO: test ValueError when paper.pdf_url is None
    pass


# ---------------------------------------------------------------------------
# download_pdf_batch / download_source_batch
# ---------------------------------------------------------------------------

class TestDownloadBatch:
    # TODO: mock download_pdf and verify it's called once per paper
    # TODO: test that dirpath is created if it doesn't exist
    # TODO: test that returned paths match what download_pdf returns
    pass


# ---------------------------------------------------------------------------
# cleanup_pdfs
# ---------------------------------------------------------------------------

class TestCleanupPdfs:
    # TODO: test that PDFs not in keep set are deleted
    # TODO: test that PDFs in keep set are preserved
    # TODO: test that non-PDF files are not touched
    # TODO: test with empty keep set (all PDFs deleted)
    # TODO: test returns list of deleted paths
    pass


# ---------------------------------------------------------------------------
# saved_pdfs_size
# ---------------------------------------------------------------------------

class TestSavedPdfsSize:
    # TODO: test returns 0 for empty set
    # TODO: test returns sum of file sizes for existing files
    # TODO: test skips paths that don't exist (no FileNotFoundError)
    pass


# ---------------------------------------------------------------------------
# extract_source
# ---------------------------------------------------------------------------

class TestExtractSource:
    # TODO: test with a valid .tar.gz containing .tex files
    # TODO: test that comments are stripped from output
    # TODO: test ordering (root-level files come before nested)
    # TODO: test returns "" for tar with no .tex files
    # TODO: test returns "" for corrupt/invalid tar
    # TODO: test that unsafe paths (starting with / or ..) are skipped
    pass

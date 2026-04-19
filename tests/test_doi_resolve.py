"""Integration tests for doi_resolve.py — covers all three resolution strategies,
the fallback chain, rate-limit handling, and edge cases."""

from __future__ import annotations

import datetime
import pytest
from unittest.mock import MagicMock, patch
from urllib.error import URLError

from sources.doi_resolve import (
    _strip_doi_url,
    _is_ratelimited,
    _try_arxiv_doi,
    _try_semantic_scholar,
    _try_crossref,
    resolve_doi,
)
from sources.base import PaperMetadata


def _make_meta(**kwargs) -> PaperMetadata:
    defaults = dict(
        paper_id="2204.12985",
        version=1,
        title="Test Paper",
        authors=["Author One"],
        published=datetime.date(2022, 4, 1),
        summary="Abstract.",
        source="arxiv",
    )
    defaults.update(kwargs)
    return PaperMetadata.model_validate(defaults)


# ---------------------------------------------------------------------------
# _strip_doi_url
# ---------------------------------------------------------------------------

class TestStripDoiUrl:
    def test_strips_https_prefix(self):
        assert _strip_doi_url("https://doi.org/10.1000/xyz") == "10.1000/xyz"

    def test_strips_http_prefix(self):
        assert _strip_doi_url("http://doi.org/10.1000/xyz") == "10.1000/xyz"

    def test_strips_dx_doi_prefix(self):
        assert _strip_doi_url("https://dx.doi.org/10.1000/xyz") == "10.1000/xyz"

    def test_bare_doi_unchanged(self):
        assert _strip_doi_url("10.1000/xyz") == "10.1000/xyz"

    def test_strips_whitespace(self):
        assert _strip_doi_url("  10.1000/xyz  ") == "10.1000/xyz"


# ---------------------------------------------------------------------------
# _is_ratelimited
# ---------------------------------------------------------------------------

class TestIsRatelimited:
    def test_detects_429(self):
        assert _is_ratelimited(Exception("HTTP 429 Too Many Requests"))

    def test_returns_false_for_other_codes(self):
        assert not _is_ratelimited(Exception("HTTP 404 Not Found"))
        assert not _is_ratelimited(Exception("Connection reset"))


# ---------------------------------------------------------------------------
# _try_arxiv_doi
# ---------------------------------------------------------------------------

class TestTryArxivDoi:
    def test_returns_none_for_non_arxiv_doi(self):
        assert _try_arxiv_doi("10.1000/xyz123") is None

    def test_returns_none_for_blank(self):
        assert _try_arxiv_doi("") is None

    def test_fetches_arxiv_for_matching_doi(self):
        mock_meta = _make_meta(paper_id="2204.12985")
        with patch("fetch_paper_metadata.fetch_paper_metadata", return_value=MagicMock()), \
             patch("sources.arxiv_source._result_to_metadata", return_value=mock_meta):
            result = _try_arxiv_doi("10.48550/arXiv.2204.12985")
        assert result is mock_meta

    def test_raises_value_error_on_429(self):
        with patch("fetch_paper_metadata.fetch_paper_metadata",
                   side_effect=Exception("429 Too Many Requests")):
            with pytest.raises(ValueError, match="rate limit"):
                _try_arxiv_doi("10.48550/arXiv.2204.12985")

    def test_returns_none_on_other_arxiv_error(self):
        with patch("fetch_paper_metadata.fetch_paper_metadata",
                   side_effect=Exception("network error")):
            result = _try_arxiv_doi("10.48550/arXiv.2204.12985")
        assert result is None

    def test_matches_case_insensitive(self):
        mock_meta = _make_meta()
        with patch("fetch_paper_metadata.fetch_paper_metadata", return_value=MagicMock()), \
             patch("sources.arxiv_source._result_to_metadata", return_value=mock_meta):
            result = _try_arxiv_doi("10.48550/ARXIV.2204.12985")
        assert result is mock_meta


# ---------------------------------------------------------------------------
# _try_semantic_scholar
# ---------------------------------------------------------------------------

class TestTrySemanticScholar:
    def test_returns_none_on_url_error(self):
        with patch("doi_resolve._fetch_url", side_effect=URLError("timeout")):
            assert _try_semantic_scholar("10.1000/xyz") is None

    def test_returns_none_on_empty_response(self):
        with patch("doi_resolve._fetch_url", return_value={}):
            assert _try_semantic_scholar("10.1000/xyz") is None

    def test_returns_none_when_no_title(self):
        with patch("doi_resolve._fetch_url", return_value={"authors": []}):
            assert _try_semantic_scholar("10.1000/xyz") is None

    def test_builds_metadata_from_s2_without_arxiv_id(self):
        s2_data = {
            "title": "A Test Paper",
            "authors": [{"name": "Jane Doe"}],
            "publicationDate": "2023-05-01",
            "abstract": "An abstract.",
            "externalIds": {},
            "url": "https://www.semanticscholar.org/paper/abc",
        }
        with patch("doi_resolve._fetch_url", return_value=s2_data):
            result = _try_semantic_scholar("10.1000/xyz")
        assert result is not None
        assert result.title == "A Test Paper"
        assert result.authors == ["Jane Doe"]
        assert result.source == "semanticscholar"
        assert result.summary == "An abstract."

    def test_fetches_arxiv_when_s2_has_arxiv_id(self):
        s2_data = {
            "title": "ArXiv Paper",
            "authors": [],
            "publicationDate": "2022-04-01",
            "abstract": None,
            "externalIds": {"ArXiv": "2204.12985"},
        }
        mock_meta = _make_meta(paper_id="2204.12985")
        with patch("doi_resolve._fetch_url", return_value=s2_data), \
             patch("fetch_paper_metadata.fetch_paper_metadata", return_value=MagicMock()), \
             patch("sources.arxiv_source._result_to_metadata", return_value=mock_meta):
            result = _try_semantic_scholar("10.1000/xyz")
        assert result is mock_meta

    def test_raises_value_error_on_arxiv_429_via_s2(self):
        s2_data = {
            "title": "ArXiv Paper",
            "authors": [],
            "externalIds": {"ArXiv": "2204.12985"},
        }
        with patch("doi_resolve._fetch_url", return_value=s2_data), \
             patch("fetch_paper_metadata.fetch_paper_metadata",
                   side_effect=Exception("429")):
            with pytest.raises(ValueError, match="rate limit"):
                _try_semantic_scholar("10.1000/xyz")

    def test_uses_year_fallback_when_no_publication_date(self):
        s2_data = {
            "title": "Old Paper",
            "authors": [],
            "publicationDate": None,
            "year": 2019,
            "abstract": None,
            "externalIds": {},
        }
        with patch("doi_resolve._fetch_url", return_value=s2_data):
            result = _try_semantic_scholar("10.1000/xyz")
        assert result is not None
        assert result.published.year == 2019

    def test_uses_today_when_no_date_or_year(self):
        s2_data = {
            "title": "Undated Paper",
            "authors": [],
            "publicationDate": None,
            "year": None,
            "abstract": None,
            "externalIds": {},
        }
        with patch("doi_resolve._fetch_url", return_value=s2_data):
            result = _try_semantic_scholar("10.1000/xyz")
        assert result is not None
        assert result.published == datetime.date.today()


# ---------------------------------------------------------------------------
# _try_crossref
# ---------------------------------------------------------------------------

class TestTryCrossref:
    def test_returns_none_on_url_error(self):
        with patch("doi_resolve._fetch_url", side_effect=URLError("timeout")):
            assert _try_crossref("10.1000/xyz") is None

    def test_returns_none_when_no_title(self):
        with patch("doi_resolve._fetch_url", return_value={"message": {"title": []}}):
            assert _try_crossref("10.1000/xyz") is None

    def test_builds_metadata_from_crossref_response(self):
        cr_data = {
            "message": {
                "title": ["A CrossRef Paper"],
                "author": [{"given": "John", "family": "Smith"}],
                "published": {"date-parts": [[2021, 3, 15]]},
                "abstract": "<jats:p>Abstract text</jats:p>",
                "container-title": ["Journal of Testing"],
                "URL": "https://doi.org/10.1000/xyz",
            }
        }
        with patch("doi_resolve._fetch_url", return_value=cr_data):
            result = _try_crossref("10.1000/xyz")
        assert result is not None
        assert result.title == "A CrossRef Paper"
        assert result.authors == ["John Smith"]
        assert result.published == datetime.date(2021, 3, 15)
        assert result.summary == "Abstract text"
        assert result.source == "crossref"

    def test_strips_html_tags_from_abstract(self):
        cr_data = {
            "message": {
                "title": ["Paper"],
                "author": [],
                "abstract": "<jats:p>Clean <b>text</b> here</jats:p>",
                "container-title": [],
            }
        }
        with patch("doi_resolve._fetch_url", return_value=cr_data):
            result = _try_crossref("10.1000/xyz")
        assert result is not None
        assert "<" not in result.summary
        assert "Clean" in result.summary

    def test_handles_malformed_date_gracefully(self):
        cr_data = {
            "message": {
                "title": ["Paper"],
                "author": [],
                "published": {"date-parts": [["bad"]]},
                "abstract": None,
                "container-title": [],
            }
        }
        with patch("doi_resolve._fetch_url", return_value=cr_data):
            result = _try_crossref("10.1000/xyz")
        assert result is not None

    def test_handles_partial_date_year_only(self):
        cr_data = {
            "message": {
                "title": ["Paper"],
                "author": [],
                "published": {"date-parts": [[2020]]},
                "abstract": None,
                "container-title": [],
            }
        }
        with patch("doi_resolve._fetch_url", return_value=cr_data):
            result = _try_crossref("10.1000/xyz")
        assert result is not None
        assert result.published.year == 2020
        assert result.published.month == 1
        assert result.published.day == 1


# ---------------------------------------------------------------------------
# resolve_doi — full fallback chain
# ---------------------------------------------------------------------------

class TestResolveDoiFallbackChain:
    def test_raises_on_empty_doi(self):
        with pytest.raises(ValueError, match="Please enter a DOI"):
            resolve_doi("")

    def test_raises_on_doi_url_that_strips_to_empty(self):
        with pytest.raises(ValueError):
            resolve_doi("https://doi.org/")

    def test_arxiv_path_used_for_arxiv_doi(self):
        mock_meta = _make_meta()
        with patch("doi_resolve._try_arxiv_doi", return_value=mock_meta) as mock_ax, \
             patch("doi_resolve._try_semantic_scholar") as mock_s2:
            result = resolve_doi("10.48550/arXiv.2204.12985")
        assert result is mock_meta
        mock_s2.assert_not_called()

    def test_falls_back_to_s2_when_arxiv_returns_none(self):
        mock_meta = _make_meta(source="semanticscholar")
        with patch("doi_resolve._try_arxiv_doi", return_value=None), \
             patch("doi_resolve._try_semantic_scholar", return_value=mock_meta), \
             patch("doi_resolve._try_crossref") as mock_cr:
            result = resolve_doi("10.1000/xyz")
        assert result is mock_meta
        mock_cr.assert_not_called()

    def test_falls_back_to_crossref_when_s2_returns_none(self):
        mock_meta = _make_meta(source="crossref")
        with patch("doi_resolve._try_arxiv_doi", return_value=None), \
             patch("doi_resolve._try_semantic_scholar", return_value=None), \
             patch("doi_resolve._try_crossref", return_value=mock_meta):
            result = resolve_doi("10.1000/xyz")
        assert result is mock_meta

    def test_raises_when_all_strategies_fail(self):
        with patch("doi_resolve._try_arxiv_doi", return_value=None), \
             patch("doi_resolve._try_semantic_scholar", return_value=None), \
             patch("doi_resolve._try_crossref", return_value=None):
            with pytest.raises(ValueError, match="Could not resolve"):
                resolve_doi("10.1000/unknown")

    def test_strips_doi_url_before_resolving(self):
        mock_meta = _make_meta()
        with patch("doi_resolve._try_arxiv_doi", return_value=None), \
             patch("doi_resolve._try_semantic_scholar", return_value=mock_meta):
            result = resolve_doi("https://doi.org/10.1000/xyz")
        assert result is mock_meta

    def test_rate_limit_error_propagates_through_chain(self):
        with patch("doi_resolve._try_arxiv_doi",
                   side_effect=ValueError("arXiv rate limit reached")):
            with pytest.raises(ValueError, match="rate limit"):
                resolve_doi("10.48550/arXiv.2204.12985")

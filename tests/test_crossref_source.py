"""Unit tests for sources/crossref_source.py — all network calls are mocked."""

from __future__ import annotations

import datetime
import pytest
from unittest.mock import MagicMock, patch
import httpx

from sources.base import PaperMetadata
from sources.crossref_source import (
    _parse_crossref_work,
    fetch_by_doi,
    search_by_title,
    CrossRefSource,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _make_msg(**kwargs) -> dict:
    """Build a minimal CrossRef work message dict."""
    return {
        "title": kwargs.get("title", ["Test Paper"]),
        "author": kwargs.get("author", [{"given": "Jane", "family": "Doe"}]),
        "published": kwargs.get("published", {"date-parts": [[2023, 6, 15]]}),
        "abstract": kwargs.get("abstract", "<p>An abstract.</p>"),
        "container-title": kwargs.get("container-title", ["Journal of Testing"]),
        "URL": kwargs.get("URL", "https://doi.org/10.1000/xyz"),
        "DOI": kwargs.get("DOI", "10.1000/xyz"),
    }


def _mock_response(status: int = 200, json_data: dict | None = None) -> MagicMock:
    resp = MagicMock()
    resp.status_code = status
    resp.json.return_value = json_data or {}
    return resp


# ---------------------------------------------------------------------------
# _parse_crossref_work
# ---------------------------------------------------------------------------

class TestParseCrossrefWork:
    def test_returns_paper_metadata(self):
        meta = _parse_crossref_work(_make_msg(), doi="10.1000/xyz")
        assert isinstance(meta, PaperMetadata)

    def test_source_is_crossref(self):
        meta = _parse_crossref_work(_make_msg(), doi="10.1000/xyz")
        assert meta.source == "crossref"

    def test_paper_id_is_doi(self):
        meta = _parse_crossref_work(_make_msg(), doi="10.1000/xyz")
        assert meta.paper_id == "10.1000/xyz"

    def test_doi_field_set(self):
        meta = _parse_crossref_work(_make_msg(), doi="10.1000/xyz")
        assert meta.doi == "10.1000/xyz"

    def test_title_extracted(self):
        meta = _parse_crossref_work(_make_msg(title=["My Title"]), doi="10.1000/xyz")
        assert meta.title == "My Title"

    def test_authors_joined(self):
        msg = _make_msg(author=[
            {"given": "Alice", "family": "Smith"},
            {"given": "Bob", "family": "Jones"},
        ])
        meta = _parse_crossref_work(msg, doi="10.1000/xyz")
        assert meta.authors == ["Alice Smith", "Bob Jones"]

    def test_author_with_only_family_name(self):
        msg = _make_msg(author=[{"family": "Einstein"}])
        meta = _parse_crossref_work(msg, doi="10.1000/xyz")
        assert meta.authors == ["Einstein"]

    def test_authors_without_name_skipped(self):
        msg = _make_msg(author=[{"given": "Alice", "family": "Smith"}, {}])
        meta = _parse_crossref_work(msg, doi="10.1000/xyz")
        assert meta.authors == ["Alice Smith"]

    def test_full_date_parsed(self):
        meta = _parse_crossref_work(_make_msg(published={"date-parts": [[2021, 3, 15]]}), doi="10.1000/xyz")
        assert meta.published == datetime.date(2021, 3, 15)

    def test_year_only_date_defaults_month_day_to_1(self):
        meta = _parse_crossref_work(_make_msg(published={"date-parts": [[2020]]}), doi="10.1000/xyz")
        assert meta.published == datetime.date(2020, 1, 1)

    def test_year_month_date_defaults_day_to_1(self):
        meta = _parse_crossref_work(_make_msg(published={"date-parts": [[2020, 7]]}), doi="10.1000/xyz")
        assert meta.published == datetime.date(2020, 7, 1)

    def test_malformed_date_falls_back_to_today(self):
        meta = _parse_crossref_work(_make_msg(published={"date-parts": [["bad"]]}), doi="10.1000/xyz")
        assert isinstance(meta.published, datetime.date)

    def test_missing_published_falls_back_to_today(self):
        msg = _make_msg()
        del msg["published"]
        meta = _parse_crossref_work(msg, doi="10.1000/xyz")
        assert isinstance(meta.published, datetime.date)

    def test_html_stripped_from_abstract(self):
        msg = _make_msg(abstract="<jats:p>Clean <b>text</b> here</jats:p>")
        meta = _parse_crossref_work(msg, doi="10.1000/xyz")
        assert "<" not in meta.summary
        assert "Clean" in meta.summary
        assert "text" in meta.summary

    def test_none_abstract_becomes_empty_string(self):
        meta = _parse_crossref_work(_make_msg(abstract=None), doi="10.1000/xyz")
        assert meta.summary == ""

    def test_container_title_becomes_category(self):
        meta = _parse_crossref_work(_make_msg(**{"container-title": ["Nature"]}), doi="10.1000/xyz")
        assert meta.category == "Nature"

    def test_empty_container_title_gives_none_category(self):
        meta = _parse_crossref_work(_make_msg(**{"container-title": []}), doi="10.1000/xyz")
        assert meta.category is None

    def test_url_from_message(self):
        meta = _parse_crossref_work(_make_msg(URL="https://doi.org/10.1000/xyz"), doi="10.1000/xyz")
        assert meta.url == "https://doi.org/10.1000/xyz"

    def test_url_falls_back_to_doi_url(self):
        msg = _make_msg(URL=None)
        msg["URL"] = None
        meta = _parse_crossref_work(msg, doi="10.5678/abc")
        assert meta.url == "https://doi.org/10.5678/abc"

    def test_version_is_always_1(self):
        meta = _parse_crossref_work(_make_msg(), doi="10.1000/xyz")
        assert meta.version == 1


# ---------------------------------------------------------------------------
# fetch_by_doi
# ---------------------------------------------------------------------------

class TestFetchByDoi:
    def _make_full_response(self, doi: str = "10.1000/xyz") -> dict:
        return {"message": _make_msg(DOI=doi)}

    def test_happy_path_returns_paper_metadata(self):
        mock_resp = _mock_response(200, self._make_full_response())
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            result = fetch_by_doi("10.1000/xyz")
        assert isinstance(result, PaperMetadata)
        assert result.source == "crossref"
        assert result.paper_id == "10.1000/xyz"

    def test_404_returns_none(self):
        mock_resp = _mock_response(404, {})
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            result = fetch_by_doi("10.1000/notfound")
        assert result is None

    def test_connection_error_returns_none(self):
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.side_effect = httpx.ConnectError("timeout")
            result = fetch_by_doi("10.1000/xyz")
        assert result is None

    def test_missing_title_returns_none(self):
        msg = _make_msg(title=[])
        mock_resp = _mock_response(200, {"message": msg})
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            result = fetch_by_doi("10.1000/xyz")
        assert result is None

    def test_empty_message_returns_none(self):
        mock_resp = _mock_response(200, {"message": {}})
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            result = fetch_by_doi("10.1000/xyz")
        assert result is None

    def test_doi_passed_as_paper_id(self):
        mock_resp = _mock_response(200, self._make_full_response(doi="10.9999/test"))
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            result = fetch_by_doi("10.9999/test")
        assert result is not None
        assert result.paper_id == "10.9999/test"


# ---------------------------------------------------------------------------
# search_by_title
# ---------------------------------------------------------------------------

class TestSearchByTitle:
    def _make_search_response(self, titles: list[str]) -> dict:
        items = [
            _make_msg(title=[t], DOI=f"10.1000/{i}")
            for i, t in enumerate(titles)
        ]
        return {"message": {"items": items}}

    def test_happy_path_returns_list(self):
        mock_resp = _mock_response(200, self._make_search_response(["Paper A", "Paper B"]))
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            results = search_by_title("neural networks")
        assert len(results) == 2
        assert all(isinstance(r, PaperMetadata) for r in results)

    def test_empty_items_returns_empty_list(self):
        mock_resp = _mock_response(200, {"message": {"items": []}})
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            results = search_by_title("xyzzy nonexistent")
        assert results == []

    def test_network_error_returns_empty_list(self):
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.side_effect = httpx.ConnectError("timeout")
            results = search_by_title("attention is all you need")
        assert results == []

    def test_non_200_returns_empty_list(self):
        mock_resp = _mock_response(503, {})
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            results = search_by_title("test query")
        assert results == []

    def test_rows_param_sent_correctly(self):
        mock_resp = _mock_response(200, {"message": {"items": []}})
        with patch("httpx.Client") as mock_client_cls:
            mock_get = mock_client_cls.return_value.__enter__.return_value.get
            mock_get.return_value = mock_resp
            search_by_title("test", limit=3)
        params = mock_get.call_args[1]["params"]
        assert params["rows"] == 3
        assert params["query.title"] == "test"

    def test_items_without_doi_are_skipped(self):
        items = [
            {"title": ["Paper With DOI"], "DOI": "10.1000/valid", "author": [], "published": {"date-parts": [[2023]]}, "abstract": None, "container-title": [], "URL": None},
            {"title": ["Paper Without DOI"]},  # no DOI
        ]
        mock_resp = _mock_response(200, {"message": {"items": items}})
        with patch("httpx.Client") as mock_client_cls:
            mock_client_cls.return_value.__enter__.return_value.get.return_value = mock_resp
            results = search_by_title("test")
        assert len(results) == 1
        assert results[0].title == "Paper With DOI"


# ---------------------------------------------------------------------------
# CrossRefSource class
# ---------------------------------------------------------------------------

class TestCrossRefSourceClass:
    def test_source_name(self):
        assert CrossRefSource.source_name == "crossref"

    def test_fetch_by_id_returns_metadata_on_success(self):
        mock_meta = PaperMetadata(
            paper_id="10.1000/xyz", version=1, title="Paper", authors=[],
            published=datetime.date(2023, 1, 1), summary="", source="crossref",
        )
        source = CrossRefSource()
        with patch("sources.crossref_source.fetch_by_doi", return_value=mock_meta):
            result = source.fetch_by_id("10.1000/xyz")
        assert result is mock_meta

    def test_fetch_by_id_raises_when_not_found(self):
        source = CrossRefSource()
        with patch("sources.crossref_source.fetch_by_doi", return_value=None):
            with pytest.raises(ValueError, match="no record found"):
                source.fetch_by_id("10.1000/notfound")

    def test_search_returns_list_on_success(self):
        mock_results = [
            PaperMetadata(
                paper_id="10.1000/a", version=1, title="Result A", authors=[],
                published=datetime.date(2023, 1, 1), summary="", source="crossref",
            )
        ]
        source = CrossRefSource()
        with patch("sources.crossref_source.search_by_title", return_value=mock_results):
            results = source.search("attention mechanism", max_results=5)
        assert results is mock_results

    def test_search_passes_max_results(self):
        source = CrossRefSource()
        with patch("sources.crossref_source.search_by_title", return_value=[]) as mock_fn:
            source.search("test", max_results=7)
        mock_fn.assert_called_once_with("test", limit=7)

    def test_search_returns_empty_list_when_no_results(self):
        source = CrossRefSource()
        with patch("sources.crossref_source.search_by_title", return_value=[]):
            results = source.search("xyzzy nonexistent")
        assert results == []

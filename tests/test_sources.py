"""Integration tests for sources/ — covers ArxivSource, OpenAlexSource,
and their helper functions including rate-limit and error scenarios."""

from __future__ import annotations

import datetime
import pytest
from unittest.mock import MagicMock, patch
import httpx

from sources.base import PaperMetadata
from sources.arxiv_source import _parse_arxiv_id, _result_to_metadata, ArxivSource
from sources.openalex_source import _reconstruct_abstract, _work_to_metadata, OpenAlexSource


# ---------------------------------------------------------------------------
# _parse_arxiv_id
# ---------------------------------------------------------------------------

class TestParseArxivId:
    def test_full_http_url_with_version(self):
        assert _parse_arxiv_id("http://arxiv.org/abs/2204.12985v4") == ("2204.12985", 4)

    def test_full_https_url_with_version(self):
        assert _parse_arxiv_id("https://arxiv.org/abs/2204.12985v2") == ("2204.12985", 2)

    def test_bare_id_with_version(self):
        assert _parse_arxiv_id("2204.12985v3") == ("2204.12985", 3)

    def test_bare_id_no_version_defaults_to_1(self):
        assert _parse_arxiv_id("2204.12985") == ("2204.12985", 1)

    def test_five_digit_id(self):
        paper_id, ver = _parse_arxiv_id("http://arxiv.org/abs/2204.123456v1")
        assert paper_id == "2204.123456"
        assert ver == 1


# ---------------------------------------------------------------------------
# _result_to_metadata
# ---------------------------------------------------------------------------

class TestResultToMetadata:
    def _make_result(self, **kwargs):
        import arxiv
        r = MagicMock(spec=arxiv.Result)
        r.entry_id = kwargs.get("entry_id", "http://arxiv.org/abs/2204.12985v1")
        r.title = kwargs.get("title", "Test Paper")
        author_mocks = []
        for name in kwargs.get("authors", ["Author One"]):
            m = MagicMock()
            m.name = name
            author_mocks.append(m)
        r.authors = author_mocks
        r.published = MagicMock()
        r.published.date.return_value = kwargs.get("published", datetime.date(2022, 4, 1))
        r.updated = None
        r.summary = kwargs.get("summary", "Abstract text.")
        r.primary_category = kwargs.get("category", "cs.AI")
        r.categories = kwargs.get("categories", ["cs.AI"])
        r.doi = kwargs.get("doi", None)
        r.journal_ref = kwargs.get("journal_ref", None)
        r.comment = kwargs.get("comment", None)
        r.pdf_url = kwargs.get("pdf_url", "https://arxiv.org/pdf/2204.12985")
        return r

    def test_basic_conversion_returns_paper_metadata(self):
        meta = _result_to_metadata(self._make_result())
        assert isinstance(meta, PaperMetadata)
        assert meta.source == "arxiv"

    def test_extracts_paper_id_from_entry_id(self):
        meta = _result_to_metadata(self._make_result(entry_id="http://arxiv.org/abs/2204.12985v1"))
        assert meta.paper_id == "2204.12985"

    def test_extracts_version_from_entry_id(self):
        meta = _result_to_metadata(self._make_result(entry_id="http://arxiv.org/abs/2204.12985v3"))
        assert meta.version == 3

    def test_extracts_all_authors(self):
        meta = _result_to_metadata(self._make_result(authors=["Alice Smith", "Bob Jones"]))
        assert meta.authors == ["Alice Smith", "Bob Jones"]

    def test_preserves_title_and_summary(self):
        meta = _result_to_metadata(self._make_result(title="My Title", summary="My Summary"))
        assert meta.title == "My Title"
        assert meta.summary == "My Summary"

    def test_preserves_category(self):
        meta = _result_to_metadata(self._make_result(category="cs.LG"))
        assert meta.category == "cs.LG"

    def test_preserves_doi_when_present(self):
        meta = _result_to_metadata(self._make_result(doi="10.1234/test"))
        assert meta.doi == "10.1234/test"

    def test_doi_is_none_when_absent(self):
        meta = _result_to_metadata(self._make_result(doi=None))
        assert meta.doi is None


# ---------------------------------------------------------------------------
# ArxivSource — rate limit and error handling
# ---------------------------------------------------------------------------

class TestArxivSource:
    def test_search_raises_on_429(self):
        source = ArxivSource()
        with patch.object(source._client, "results",
                          side_effect=Exception("HTTP Error 429 Too Many Requests")):
            with pytest.raises(Exception, match="429"):
                source.search("attention mechanism")

    def test_fetch_by_id_raises_value_error_when_not_found(self):
        source = ArxivSource()
        with patch.object(source._client, "results", return_value=iter([])):
            with pytest.raises(ValueError, match="not found"):
                source.fetch_by_id("9999.99999")

    def test_fetch_by_id_raises_on_network_error(self):
        source = ArxivSource()
        with patch.object(source._client, "results",
                          side_effect=Exception("Connection refused")):
            with pytest.raises(Exception):
                source.fetch_by_id("2204.12985")


# ---------------------------------------------------------------------------
# _reconstruct_abstract
# ---------------------------------------------------------------------------

class TestReconstructAbstract:
    def test_reconstructs_simple_sentence(self):
        inv = {"The": [0], "quick": [1], "fox": [2]}
        assert _reconstruct_abstract(inv) == "The quick fox"

    def test_handles_word_at_multiple_positions(self):
        inv = {"the": [0, 3], "cat": [1], "sat": [2]}
        result = _reconstruct_abstract(inv)
        assert result is not None
        words = result.split()
        assert words[0] == "the"
        assert words[1] == "cat"
        assert words[2] == "sat"
        assert words[3] == "the"

    def test_returns_none_for_none_input(self):
        assert _reconstruct_abstract(None) is None

    def test_returns_none_for_empty_dict(self):
        assert _reconstruct_abstract({}) is None

    def test_preserves_punctuation_as_words(self):
        inv = {"Hello": [0], "world.": [1]}
        assert _reconstruct_abstract(inv) == "Hello world."


# ---------------------------------------------------------------------------
# _work_to_metadata
# ---------------------------------------------------------------------------

class TestWorkToMetadata:
    def _make_work(self, **kwargs) -> dict:
        return {
            "id": kwargs.get("id", "https://openalex.org/W3123456789"),
            "title": kwargs.get("title", "OpenAlex Paper"),
            "authorships": kwargs.get("authorships", [
                {"author": {"display_name": "Jane Doe"}}
            ]),
            "publication_date": kwargs.get("publication_date", "2023-06-01"),
            "doi": kwargs.get("doi", None),
            "primary_topic": kwargs.get("primary_topic", None),
            "abstract_inverted_index": kwargs.get("abstract_inverted_index", None),
        }

    def test_basic_conversion(self):
        meta = _work_to_metadata(self._make_work())
        assert isinstance(meta, PaperMetadata)
        assert meta.paper_id == "W3123456789"
        assert meta.source == "openalex"
        assert meta.title == "OpenAlex Paper"
        assert meta.version == 1

    def test_extracts_openalex_id_from_url(self):
        meta = _work_to_metadata(self._make_work(id="https://openalex.org/W9876543210"))
        assert meta.paper_id == "W9876543210"

    def test_extracts_authors(self):
        work = self._make_work(authorships=[
            {"author": {"display_name": "Alice"}},
            {"author": {"display_name": "Bob"}},
        ])
        meta = _work_to_metadata(work)
        assert meta.authors == ["Alice", "Bob"]

    def test_skips_authors_without_display_name(self):
        work = self._make_work(authorships=[
            {"author": {"display_name": "Alice"}},
            {"author": {}},
        ])
        meta = _work_to_metadata(work)
        assert meta.authors == ["Alice"]

    def test_extracts_category_from_primary_topic(self):
        work = self._make_work(primary_topic={
            "subfield": {"display_name": "Machine Learning"}
        })
        meta = _work_to_metadata(work)
        assert meta.category == "Machine Learning"

    def test_category_is_none_when_no_primary_topic(self):
        meta = _work_to_metadata(self._make_work(primary_topic=None))
        assert meta.category is None

    def test_parses_publication_date(self):
        meta = _work_to_metadata(self._make_work(publication_date="2021-09-15"))
        assert meta.published == datetime.date(2021, 9, 15)

    def test_falls_back_to_today_on_bad_date(self):
        meta = _work_to_metadata(self._make_work(publication_date="not-a-date"))
        assert isinstance(meta.published, datetime.date)

    def test_falls_back_to_today_on_empty_date(self):
        meta = _work_to_metadata(self._make_work(publication_date=""))
        assert isinstance(meta.published, datetime.date)

    def test_raises_on_missing_id(self):
        work = self._make_work(id="")
        with pytest.raises(ValueError, match="no valid ID"):
            _work_to_metadata(work)

    def test_reconstructs_abstract_from_inverted_index(self):
        work = self._make_work(abstract_inverted_index={"Hello": [0], "world": [1]})
        meta = _work_to_metadata(work)
        assert meta.summary == "Hello world"

    def test_uses_doi_as_url_when_available(self):
        work = self._make_work(doi="https://doi.org/10.1000/xyz")
        meta = _work_to_metadata(work)
        assert meta.url == "https://doi.org/10.1000/xyz"
        assert meta.doi == "https://doi.org/10.1000/xyz"


# ---------------------------------------------------------------------------
# OpenAlexSource — HTTP mocking
# ---------------------------------------------------------------------------

def _mock_work(work_id: str = "https://openalex.org/W1111",
               title: str = "Paper One",
               publication_date: str = "2023-01-01") -> dict:
    return {
        "id": work_id,
        "title": title,
        "authorships": [],
        "publication_date": publication_date,
        "doi": None,
        "primary_topic": None,
        "abstract_inverted_index": None,
    }


class TestOpenAlexSourceSearch:

    def test_search_returns_paper_metadata_list(self):
        mock_resp = MagicMock()
        mock_resp.raise_for_status.return_value = None
        mock_resp.json.return_value = {"results": [_mock_work()]}
        source = OpenAlexSource()
        with patch.object(source._http, "get", return_value=mock_resp):
            results = source.search("machine learning")
        assert len(results) == 1
        assert results[0].title == "Paper One"
        assert results[0].source == "openalex"

    def test_search_returns_empty_list_when_no_results(self):
        mock_resp = MagicMock()
        mock_resp.raise_for_status.return_value = None
        mock_resp.json.return_value = {"results": []}
        source = OpenAlexSource()
        with patch.object(source._http, "get", return_value=mock_resp):
            results = source.search("xyzzy nonexistent query")
        assert results == []

    def test_search_raises_value_error_on_connection_error(self):
        source = OpenAlexSource()
        with patch.object(source._http, "get",
                          side_effect=httpx.ConnectError("timeout")):
            with pytest.raises(ValueError, match="OpenAlex search failed"):
                source.search("test query")

    def test_search_raises_value_error_on_http_error(self):
        mock_resp = MagicMock()
        mock_resp.raise_for_status.side_effect = httpx.HTTPStatusError(
            "503", request=MagicMock(), response=MagicMock()
        )
        source = OpenAlexSource()
        with patch.object(source._http, "get", return_value=mock_resp):
            with pytest.raises(ValueError, match="OpenAlex search failed"):
                source.search("test query")

    def test_search_passes_correct_params(self):
        mock_resp = MagicMock()
        mock_resp.raise_for_status.return_value = None
        mock_resp.json.return_value = {"results": []}
        source = OpenAlexSource()
        with patch.object(source._http, "get", return_value=mock_resp) as mock_get:
            source.search("neural networks", max_results=5)
        params = mock_get.call_args[1]["params"]
        assert params["search"] == "neural networks"
        assert params["per_page"] == 5


class TestOpenAlexSourceFetchById:
    def test_fetch_by_id_returns_paper(self):
        mock_resp = MagicMock()
        mock_resp.raise_for_status.return_value = None
        mock_resp.json.return_value = _mock_work(
            work_id="https://openalex.org/W9999",
            title="Fetched Paper",
            publication_date="2022-06-15",
        )
        source = OpenAlexSource()
        with patch.object(source._http, "get", return_value=mock_resp):
            result = source.fetch_by_id("W9999")
        assert result.title == "Fetched Paper"
        assert result.paper_id == "W9999"

    def test_fetch_by_id_raises_value_error_on_error(self):
        source = OpenAlexSource()
        with patch.object(source._http, "get",
                          side_effect=httpx.ConnectError("timeout")):
            with pytest.raises(ValueError, match="OpenAlex fetch failed"):
                source.fetch_by_id("W9999")

    def test_fetch_by_id_uses_full_url_when_provided(self):
        mock_resp = MagicMock()
        mock_resp.raise_for_status.return_value = None
        mock_resp.json.return_value = _mock_work(
            work_id="https://openalex.org/W1234",
            title="Via URL",
            publication_date="2021-01-01",
        )
        source = OpenAlexSource()
        with patch.object(source._http, "get", return_value=mock_resp) as mock_get:
            source.fetch_by_id("https://openalex.org/W1234")
        called_url = mock_get.call_args[0][0]
        assert "openalex.org" in called_url

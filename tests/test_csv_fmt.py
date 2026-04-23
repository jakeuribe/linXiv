"""Tests for formats/csv_fmt.py — import/export round-trips and edge cases."""

from __future__ import annotations

import csv
import datetime
import io
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from formats.csv_fmt import CSVFormat, TSVFormat

_fmt = CSVFormat()
_tsv = TSVFormat()


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------

def _make_paper(**overrides) -> dict:
    base = {
        "paper_id":  "2301.00001",
        "title":     "Test Paper",
        "authors":   ["Alice Author", "Bob Builder"],
        "category":  "cs.LG",
        "tags":      ["ml", "nlp"],
        "published": datetime.date(2023, 1, 1),
        "has_pdf":   False,
    }
    base.update(overrides)
    return base


def _write_temp_csv(content: str, suffix: str = ".csv") -> str:
    f = tempfile.NamedTemporaryFile(suffix=suffix, mode="w", delete=False,
                                   encoding="utf-8", newline="")
    f.write(content)
    f.close()
    return f.name


# ---------------------------------------------------------------------------
# export_papers
# ---------------------------------------------------------------------------

class TestExportPapers:
    def test_output_has_header(self):
        out = _fmt.export_papers([_make_paper()])
        reader = csv.DictReader(io.StringIO(out))
        assert set(reader.fieldnames or []) >= {"paper_id", "title", "authors", "category", "tags", "published", "has_pdf"}

    def test_single_paper_fields(self):
        out = _fmt.export_papers([_make_paper()])
        row = next(csv.DictReader(io.StringIO(out)))
        assert row["paper_id"] == "2301.00001"
        assert row["title"] == "Test Paper"
        assert row["published"] == "2023-01-01"
        assert row["has_pdf"] == "N"

    def test_authors_semicolon_separated(self):
        out = _fmt.export_papers([_make_paper()])
        row = next(csv.DictReader(io.StringIO(out)))
        assert row["authors"] == "Alice Author; Bob Builder"

    def test_tags_comma_separated(self):
        out = _fmt.export_papers([_make_paper()])
        row = next(csv.DictReader(io.StringIO(out)))
        assert row["tags"] == "ml, nlp"

    def test_has_pdf_true(self):
        out = _fmt.export_papers([_make_paper(has_pdf=True)])
        row = next(csv.DictReader(io.StringIO(out)))
        assert row["has_pdf"] == "Y"

    def test_date_as_datetime_object(self):
        out = _fmt.export_papers([_make_paper(published=datetime.datetime(2022, 3, 5))])
        row = next(csv.DictReader(io.StringIO(out)))
        assert row["published"].startswith("2022-03-05")

    def test_multiple_papers_row_count(self):
        papers = [_make_paper(paper_id="a"), _make_paper(paper_id="b")]
        out = _fmt.export_papers(papers)
        rows = list(csv.DictReader(io.StringIO(out)))
        assert len(rows) == 2

    def test_empty_list_only_header(self):
        out = _fmt.export_papers([])
        rows = list(csv.DictReader(io.StringIO(out)))
        assert rows == []


# ---------------------------------------------------------------------------
# import_file
# ---------------------------------------------------------------------------

class TestImportFile:
    def test_basic_fields(self):
        content = "paper_id,title,authors,category,tags,published,has_pdf\n2301.00001,My Paper,A. Author,cs.LG,ml,2023-01-01,N\n"
        path = _write_temp_csv(content)
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 1
            p = papers[0]
            assert p.paper_id == "2301.00001"
            assert p.title == "My Paper"
            assert p.category == "cs.LG"
            assert p.published == datetime.date(2023, 1, 1)
        finally:
            os.unlink(path)

    def test_authors_parsed_from_semicolons(self):
        content = "paper_id,title,authors,category,tags,published,has_pdf\n1,T,Alice; Bob,,,2020-01-01,N\n"
        path = _write_temp_csv(content)
        try:
            papers = _fmt.import_file(path)
            assert papers[0].authors == ["Alice", "Bob"]
        finally:
            os.unlink(path)

    def test_tags_parsed_from_commas(self):
        content = "paper_id,title,authors,category,tags,published,has_pdf\n1,T,,,,2020-01-01,N\n"
        content = "paper_id,title,authors,category,tags,published,has_pdf\n1,T,,cs.AI,\"ml, nlp\",2020-01-01,N\n"
        path = _write_temp_csv(content)
        try:
            papers = _fmt.import_file(path)
            assert papers[0].tags == ["ml", "nlp"]
        finally:
            os.unlink(path)

    def test_skips_rows_without_paper_id(self):
        content = "paper_id,title,authors,category,tags,published,has_pdf\n,No ID paper,,,,2020-01-01,N\n1,Has ID,,,,2021-01-01,N\n"
        path = _write_temp_csv(content)
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 1
            assert papers[0].paper_id == "1"
        finally:
            os.unlink(path)

    def test_missing_date_uses_fallback(self):
        content = "paper_id,title,authors,category,tags,published,has_pdf\n1,T,,,,,N\n"
        path = _write_temp_csv(content)
        try:
            papers = _fmt.import_file(path)
            assert papers[0].published == datetime.date(1900, 1, 1)
        finally:
            os.unlink(path)

    def test_multiple_rows(self):
        content = "paper_id,title,authors,category,tags,published,has_pdf\n1,A,,,, 2021-01-01,N\n2,B,,,,2022-01-01,N\n"
        path = _write_temp_csv(content)
        try:
            papers = _fmt.import_file(path)
            assert len(papers) == 2
        finally:
            os.unlink(path)


# ---------------------------------------------------------------------------
# TSV
# ---------------------------------------------------------------------------

class TestTSVFormat:
    def test_export_uses_tab_delimiter(self):
        out = _tsv.export_papers([_make_paper()])
        assert "\t" in out

    def test_tsv_round_trip(self):
        exported = _tsv.export_papers([_make_paper()])
        path = _write_temp_csv(exported, suffix=".tsv")
        try:
            papers = _tsv.import_file(path)
            assert len(papers) == 1
            assert papers[0].paper_id == "2301.00001"
        finally:
            os.unlink(path)


# ---------------------------------------------------------------------------
# round-trip
# ---------------------------------------------------------------------------

class TestRoundTrip:
    def _round_trip(self, paper: dict):
        exported = _fmt.export_papers([paper])
        path = _write_temp_csv(exported)
        try:
            return _fmt.import_file(path)
        finally:
            os.unlink(path)

    def test_preserves_paper_id(self):
        assert self._round_trip(_make_paper())[0].paper_id == "2301.00001"

    def test_preserves_title(self):
        assert self._round_trip(_make_paper())[0].title == "Test Paper"

    def test_preserves_authors(self):
        rt = self._round_trip(_make_paper())
        assert rt[0].authors == ["Alice Author", "Bob Builder"]

    def test_preserves_published(self):
        assert self._round_trip(_make_paper())[0].published == datetime.date(2023, 1, 1)

    def test_preserves_category(self):
        assert self._round_trip(_make_paper())[0].category == "cs.LG"

    def test_preserves_tags(self):
        assert self._round_trip(_make_paper())[0].tags == ["ml", "nlp"]

    def test_two_papers_round_trip(self):
        papers = [_make_paper(paper_id="a", title="A"), _make_paper(paper_id="b", title="B")]
        exported = _fmt.export_papers(papers)
        path = _write_temp_csv(exported)
        try:
            rt = _fmt.import_file(path)
        finally:
            os.unlink(path)
        assert {p.paper_id for p in rt} == {"a", "b"}

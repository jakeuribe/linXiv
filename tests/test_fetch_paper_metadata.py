"""Exhaustive tests for the rate-limit machinery in sources/fetch_paper_metadata.py.

Covers _check_ratelimit, _record_ratelimit, and _arxiv_call in isolation,
including all boundary conditions and error-message patterns.
"""

from __future__ import annotations

import importlib
import os
from datetime import datetime as real_datetime, timedelta
from unittest.mock import MagicMock
from types import ModuleType

import pytest

# sources/__init__.py exports a function named fetch_paper_metadata, which
# shadows the submodule in the package namespace. Use importlib to get the
# actual module object instead of the function.
fpm: ModuleType = importlib.import_module("sources.fetch_paper_metadata")


# ---------------------------------------------------------------------------
# Fixture: redirect _RATELIMIT_FILE to a temp path for every test
# ---------------------------------------------------------------------------

@pytest.fixture(autouse=True)
def temp_ratelimit(tmp_path, monkeypatch):
    monkeypatch.setattr(fpm, "_RATELIMIT_FILE", str(tmp_path / ".arxiv_ratelimit"))


def _write_ratelimit(ts: real_datetime) -> None:
    with open(fpm._RATELIMIT_FILE, "w") as f:
        f.write(ts.isoformat())


def _mock_now(monkeypatch, now: real_datetime) -> None:
    """Patch datetime.now() inside fetch_paper_metadata to return `now`."""
    mock_dt = MagicMock()
    mock_dt.now.return_value = now
    mock_dt.fromisoformat = real_datetime.fromisoformat
    monkeypatch.setattr(fpm, "datetime", mock_dt)


# ---------------------------------------------------------------------------
# _check_ratelimit — no file
# ---------------------------------------------------------------------------

class TestCheckRatelimitNoFile:
    def test_no_file_returns_immediately(self, monkeypatch):
        sleep_calls: list[float] = []

        def record(s: float) -> None:
            sleep_calls.append(s)

        monkeypatch.setattr(fpm.time, "sleep", record)
        fpm._check_ratelimit()
        assert sleep_calls == []

    def test_no_file_does_not_create_one(self):
        fpm._check_ratelimit()
        assert not os.path.exists(fpm._RATELIMIT_FILE)


# ---------------------------------------------------------------------------
# _check_ratelimit — file present, various ages
# ---------------------------------------------------------------------------

class TestCheckRatelimitFileAge:
    _BASE = real_datetime(2024, 6, 15, 12, 0, 0)

    def _run(self, monkeypatch, seconds_ago: float) -> list[float]:
        last = self._BASE
        now = last + timedelta(seconds=seconds_ago)
        _write_ratelimit(last)
        _mock_now(monkeypatch, now)
        calls: list[float] = []

        def record(s: float) -> None:
            calls.append(s)

        monkeypatch.setattr(fpm.time, "sleep", record)
        fpm._check_ratelimit()
        return calls

    def test_old_file_61s_no_sleep(self, monkeypatch):
        assert self._run(monkeypatch, 61.0) == []

    def test_old_file_exactly_60s_no_sleep(self, monkeypatch):
        # remaining = 60 - 60 = 0; condition is remaining > 0
        assert self._run(monkeypatch, 60.0) == []

    def test_old_file_60_001s_no_sleep(self, monkeypatch):
        assert self._run(monkeypatch, 60.001) == []

    def test_recent_file_30s_sleeps_30s(self, monkeypatch):
        calls = self._run(monkeypatch, 30.0)
        assert len(calls) == 1
        assert abs(calls[0] - 30.0) < 0.01

    def test_recent_file_1s_sleeps_59s(self, monkeypatch):
        calls = self._run(monkeypatch, 1.0)
        assert len(calls) == 1
        assert abs(calls[0] - 59.0) < 0.01

    def test_recent_file_59_999s_sleeps_fractional(self, monkeypatch):
        calls = self._run(monkeypatch, 59.999)
        assert len(calls) == 1
        assert 0 < calls[0] < 0.1

    def test_very_recent_file_0s_sleeps_60s(self, monkeypatch):
        calls = self._run(monkeypatch, 0.0)
        assert len(calls) == 1
        assert abs(calls[0] - 60.0) < 0.01

    def test_sleep_called_exactly_once(self, monkeypatch):
        calls = self._run(monkeypatch, 1.0)
        assert len(calls) == 1

    def test_very_old_file_1000s_no_sleep(self, monkeypatch):
        assert self._run(monkeypatch, 1000.0) == []


# ---------------------------------------------------------------------------
# _check_ratelimit — malformed file content (documents real behavior)
# ---------------------------------------------------------------------------

class TestCheckRatelimitMalformedFile:
    def test_empty_file_raises(self, monkeypatch):
        with open(fpm._RATELIMIT_FILE, "w") as f:
            f.write("")
        monkeypatch.setattr(fpm.time, "sleep", MagicMock())
        with pytest.raises(ValueError):
            fpm._check_ratelimit()

    def test_whitespace_only_raises(self, monkeypatch):
        with open(fpm._RATELIMIT_FILE, "w") as f:
            f.write("   \n  ")
        monkeypatch.setattr(fpm.time, "sleep", MagicMock())
        with pytest.raises(ValueError):
            fpm._check_ratelimit()

    def test_garbage_content_raises(self, monkeypatch):
        with open(fpm._RATELIMIT_FILE, "w") as f:
            f.write("not-a-datetime")
        monkeypatch.setattr(fpm.time, "sleep", MagicMock())
        with pytest.raises(ValueError):
            fpm._check_ratelimit()


# ---------------------------------------------------------------------------
# _record_ratelimit
# ---------------------------------------------------------------------------

class TestRecordRatelimit:
    def test_creates_file(self):
        assert not os.path.exists(fpm._RATELIMIT_FILE)
        fpm._record_ratelimit()
        assert os.path.exists(fpm._RATELIMIT_FILE)

    def test_file_contains_valid_iso_datetime(self):
        fpm._record_ratelimit()
        with open(fpm._RATELIMIT_FILE) as f:
            content = f.read().strip()
        parsed = real_datetime.fromisoformat(content)
        assert isinstance(parsed, real_datetime)

    def test_recorded_time_is_recent(self):
        before = real_datetime.now()
        fpm._record_ratelimit()
        after = real_datetime.now()
        with open(fpm._RATELIMIT_FILE) as f:
            recorded = real_datetime.fromisoformat(f.read().strip())
        assert before <= recorded <= after

    def test_overwrites_existing_file(self):
        _write_ratelimit(real_datetime(2000, 1, 1))
        fpm._record_ratelimit()
        with open(fpm._RATELIMIT_FILE) as f:
            recorded = real_datetime.fromisoformat(f.read().strip())
        assert recorded.year >= 2024

    def test_can_be_read_back_by_check(self, monkeypatch):
        fpm._record_ratelimit()
        calls: list[float] = []
        monkeypatch.setattr(fpm.time, "sleep", lambda s: calls.append(s))
        # Don't mock datetime.now so it uses real time — the file was just
        # written, so remaining should be ~60s and sleep should be called.
        fpm._check_ratelimit()
        assert len(calls) == 1
        assert 59.0 < calls[0] <= 60.0


# ---------------------------------------------------------------------------
# _arxiv_call — success path
# ---------------------------------------------------------------------------

class TestArxivCallSuccess:
    def test_returns_fn_result(self, monkeypatch):
        monkeypatch.setattr(fpm, "_check_ratelimit", lambda: None)
        result = fpm._arxiv_call(lambda: "the result")
        assert result == "the result"

    def test_calls_check_ratelimit_before_fn(self, monkeypatch):
        order: list[str] = []
        monkeypatch.setattr(fpm, "_check_ratelimit", lambda: order.append("check"))
        fpm._arxiv_call(lambda: order.append("fn") or "ok")
        assert order == ["check", "fn"]

    def test_does_not_record_ratelimit_on_success(self, monkeypatch):
        monkeypatch.setattr(fpm, "_check_ratelimit", lambda: None)
        recorded = []
        monkeypatch.setattr(fpm, "_record_ratelimit", lambda: recorded.append(True))
        fpm._arxiv_call(lambda: "ok")
        assert recorded == []

    def test_passes_fn_return_value_through(self, monkeypatch):
        monkeypatch.setattr(fpm, "_check_ratelimit", lambda: None)
        obj = {"key": "value"}
        assert fpm._arxiv_call(lambda: obj) is obj


# ---------------------------------------------------------------------------
# _arxiv_call — 429 detection
# ---------------------------------------------------------------------------

class TestArxivCall429:
    def _call_with_error(self, monkeypatch, msg: str) -> tuple[bool, Exception]:
        monkeypatch.setattr(fpm, "_check_ratelimit", lambda: None)
        recorded = []
        monkeypatch.setattr(fpm, "_record_ratelimit", lambda: recorded.append(True))
        exc = RuntimeError(msg)
        with pytest.raises(RuntimeError) as info:
            fpm._arxiv_call(lambda: (_ for _ in ()).throw(exc))
        return bool(recorded), info.value

    def test_429_in_message_records_ratelimit(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "429")
        assert recorded

    def test_http_429_records_ratelimit(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "HTTP 429 Too Many Requests")
        assert recorded

    def test_429_embedded_records_ratelimit(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "Error code: 429; rate limited")
        assert recorded

    def test_exception_is_always_reraised_on_429(self, monkeypatch):
        _, exc = self._call_with_error(monkeypatch, "429")
        assert str(exc) == "429"

    def test_404_does_not_record_ratelimit(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "404 Not Found")
        assert not recorded

    def test_500_does_not_record_ratelimit(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "500 Internal Server Error")
        assert not recorded

    def test_403_does_not_record_ratelimit(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "403 Forbidden")
        assert not recorded

    def test_connection_error_no_record(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "Connection refused")
        assert not recorded

    def test_empty_message_no_record(self, monkeypatch):
        recorded, _ = self._call_with_error(monkeypatch, "")
        assert not recorded

    def test_exception_always_propagates_non_429(self, monkeypatch):
        _, exc = self._call_with_error(monkeypatch, "something else")
        assert str(exc) == "something else"

    def test_record_called_exactly_once_on_429(self, monkeypatch):
        monkeypatch.setattr(fpm, "_check_ratelimit", lambda: None)
        record_calls = []
        monkeypatch.setattr(fpm, "_record_ratelimit", lambda: record_calls.append(True))
        with pytest.raises(RuntimeError):
            fpm._arxiv_call(lambda: (_ for _ in ()).throw(RuntimeError("429")))
        assert len(record_calls) == 1


# ---------------------------------------------------------------------------
# _arxiv_call — ratelimit check integration
# ---------------------------------------------------------------------------

class TestArxivCallRatelimitIntegration:
    def test_ratelimit_active_causes_sleep_before_fn(self, monkeypatch):
        """With a recent ratelimit file, sleep happens before the fn is called."""
        last = real_datetime(2024, 6, 15, 12, 0, 0)
        _write_ratelimit(last)
        now = last + timedelta(seconds=10)
        _mock_now(monkeypatch, now)

        order: list[str] = []
        monkeypatch.setattr(fpm.time, "sleep", lambda s: order.append(f"sleep({s:.0f})"))

        fpm._arxiv_call(lambda: order.append("fn") or "ok")

        assert order[0].startswith("sleep(")
        assert order[1] == "fn"

    def test_429_then_check_on_next_call(self, monkeypatch):
        """After a 429, the next _arxiv_call will see the recorded ratelimit file."""
        sleep_calls: list[float] = []
        monkeypatch.setattr(fpm.time, "sleep", lambda s: sleep_calls.append(s))

        # First call: 429
        with pytest.raises(RuntimeError):
            fpm._arxiv_call(lambda: (_ for _ in ()).throw(RuntimeError("429")))

        assert os.path.exists(fpm._RATELIMIT_FILE)

        # Second call: file is very recent, so sleep should be called
        fpm._arxiv_call(lambda: "ok")

        assert any(s > 50 for s in sleep_calls), "Expected ~60s sleep on second call"

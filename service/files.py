from __future__ import annotations

import ipaddress
import os
import re
import socket
import uuid
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

from storage.paths import pdf_dir as _pdf_dir

_UNSAFE_FNAME_RE = re.compile(r'[/\\:*?"<>|]')
_ALLOWED_SCHEMES = {"http", "https"}
_ALLOWED_CONTENT_TYPES = {"application/pdf", "application/octet-stream"}
_DOWNLOAD_TIMEOUT = 30
_CHUNK_SIZE = 65536
_MAX_PDF_BYTES = 200 * 1024 * 1024  # 200 MB


def _safe_name(paper_id: str | int) -> str:
    return _UNSAFE_FNAME_RE.sub("_", str(paper_id))


class _SafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    # Allow redirects to public hosts only; blocks open-redirect SSRF via 302 to 169.254.x.x etc.
    # DNS rebinding between validation and connect is a known remaining gap with urllib.
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[override]
        parsed = urlparse(newurl)
        if parsed.scheme.lower() not in _ALLOWED_SCHEMES:
            raise ValueError(f"Redirect to {newurl!r} denied: scheme {parsed.scheme!r} not allowed.")
        if not _is_safe_host(parsed.hostname or ""):
            raise ValueError(f"Redirect to {newurl!r} denied: host resolves to a disallowed range.")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


_opener = urllib.request.build_opener(_SafeRedirectHandler())


def _is_safe_host(hostname: str) -> bool:
    """Return False if the hostname resolves to any non-public address."""
    if not hostname:
        return False
    try:
        for info in socket.getaddrinfo(hostname, None):
            addr = ipaddress.ip_address(info[4][0])
            if (
                addr.is_private
                or addr.is_loopback
                or addr.is_link_local
                or addr.is_reserved
                or addr.is_multicast
                or addr.is_unspecified
            ):
                return False
        return True
    except (socket.gaierror, ValueError):
        return False


def _pdf_file(paper_id: str | int, version: int) -> Path:
    return _pdf_dir() / f"{_safe_name(paper_id)}v{version}.pdf"


def pdf_path(paper_id: str | int, version: int, custom_path: str | None = None) -> str | None:
    """Return local path to PDF if it exists, else None. Checks custom_path first."""
    if custom_path and Path(custom_path).is_file():
        return custom_path
    std = _pdf_file(paper_id, version)
    return str(std) if std.is_file() else None


def download_pdf(paper_id: str | int, version: int, url: str) -> str:
    """Download PDF to managed pdf_dir. Returns local path. Raises on failure or bad input."""
    parsed = urlparse(url)
    scheme = parsed.scheme.lower()
    if scheme not in _ALLOWED_SCHEMES:
        raise ValueError(f"Unsafe URL scheme {scheme!r}. Only http/https are allowed.")
    if not _is_safe_host(parsed.hostname or ""):
        raise ValueError(f"URL host {parsed.hostname!r} resolves to a disallowed network range.")
    dest = _pdf_file(paper_id, version)
    if dest.exists():
        return str(dest)
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(f".{uuid.uuid4().hex}.tmp")
    try:
        with _opener.open(url, timeout=_DOWNLOAD_TIMEOUT) as resp:
            content_type = resp.headers.get("content-type", "").split(";")[0].strip().lower()
            if content_type and content_type not in _ALLOWED_CONTENT_TYPES:
                raise ValueError(f"Unexpected Content-Type {content_type!r}; expected PDF.")
            declared = resp.headers.get("content-length")
            if declared:
                try:
                    declared_size = int(declared)
                except ValueError:
                    declared_size = None
                if declared_size is not None and declared_size > _MAX_PDF_BYTES:
                    raise ValueError(f"File too large ({declared_size} bytes; limit {_MAX_PDF_BYTES}).")
            total = 0
            with tmp.open("wb") as fh:
                while chunk := resp.read(_CHUNK_SIZE):
                    total += len(chunk)
                    if total > _MAX_PDF_BYTES:
                        raise ValueError(f"Download exceeded {_MAX_PDF_BYTES} byte limit.")
                    fh.write(chunk)
        tmp.replace(dest)
        return str(dest)
    except Exception as exc:
        print(f"[files] download_pdf failed: {exc}")
        tmp.unlink(missing_ok=True)
        raise


def pdf_storage_mb() -> float:
    """Total size of all managed PDFs in MB."""
    d = _pdf_dir()
    if not d.exists():
        return 0.0
    total = 0
    for entry in os.scandir(d):
        if entry.name.endswith(".pdf"):
            try:
                total += entry.stat().st_size
            except FileNotFoundError:
                pass
    return total / (1024 * 1024)


def delete_pdf(path: str) -> bool:
    """Delete a PDF only if it lives inside the managed pdf_dir. Returns True if deleted."""
    managed = _pdf_dir().resolve()
    target = Path(path).resolve()
    if not target.is_relative_to(managed):
        return False
    target.unlink(missing_ok=True)
    return True


def managed_pdf_dir() -> str:
    """Return the managed pdf_dir as a string. For use by download workers only."""
    return str(_pdf_dir())

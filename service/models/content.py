from __future__ import annotations

from dataclasses import dataclass


@dataclass
class ContentDetails:
    content_fk:   int
    content_text: str | None   = None
    content_file: bytes | None = None
from __future__ import annotations

from dataclasses import dataclass


@dataclass
class TagDetails:
    tag_id: int
    label:  str | None = None
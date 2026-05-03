from dataclasses import dataclass, field

from datetime import datetime
from typing import Optional

from storage.projects import (
    Q,
    Status,
)

@dataclass
class Paper:
    paper_id        : int
    source_id       : str | None
    source_version  : int | None

@dataclass 
class Papers:
    paper_ids       : list[int|None]
    project_ids     : list[int|None]
    tags            : list[str|None] # not SELECT from, SELECT LIKE

@dataclass 
class PaperDetails(Paper):
    abstract            : str|None
    released            : datetime|None
    updated_at_source   : datetime|None = None
    note_ids            : list[int|None]

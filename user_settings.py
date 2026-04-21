from __future__ import annotations

import json
from pathlib import Path
from typing import Any

_DEFAULTS_PATH = Path(__file__).resolve().parent / "formats" / "default_settings.json"
_USER_PATH = Path(__file__).resolve().parent / "user_settings.json"

_defaults: dict[str, Any] = json.loads(_DEFAULTS_PATH.read_text())
_settings: dict[str, Any] = {}


def _load() -> None:
    global _settings
    if _USER_PATH.exists():
        _settings = json.loads(_USER_PATH.read_text())
    else:
        _settings = {}


def save() -> None:
    _USER_PATH.write_text(json.dumps(_settings, indent=2))


def get(key: str) -> Any:
    if not _settings:
        _load()
    return _settings.get(key, _defaults.get(key))


def set(key: str, value: Any) -> None:
    if not _settings and _USER_PATH.exists():
        _load()
    _settings[key] = value
    save()


def all_settings() -> dict[str, Any]:
    if not _settings and _USER_PATH.exists():
        _load()
    return {**_defaults, **_settings}


_load()

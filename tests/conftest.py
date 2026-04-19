"""Shared fixtures for the test suite."""
import pytest
import sys
import os

# Make sure the project root is on the path so db/projects can be imported.
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


@pytest.fixture()
def tmp_db(tmp_path, monkeypatch):
    """
    Redirect all DB calls to a fresh temporary SQLite file for the duration of
    a test.

    Both db._connect and projects._connect (which is the same function object
    imported into projects via `from db import _connect`) need to be patched so
    that every call in either module ends up at the temp DB.
    """
    import storage.db as db
    import storage.projects as projects
    import storage.notes as notes

    db_file = str(tmp_path / "test.db")

    real_connect = db._connect

    def patched_connect(db_path=None):
        del db_path
        return real_connect(db_file)

    monkeypatch.setattr(db, "_connect", patched_connect)
    monkeypatch.setattr(projects, "_connect", patched_connect)
    monkeypatch.setattr(notes, "_connect", patched_connect)

    # Initialise the schema in the temp DB.
    db.init_db()
    projects.init_projects_db()
    notes.init_notes_db()

    return db_file

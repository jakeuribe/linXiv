-- PDF text-highlight annotation (Zotero-style): an ANCHOR locating the highlight
-- on a paper's PDF plus an optional written COMMENT ("" = highlight-only).
-- SOURCE_FK is required; an annotation always belongs to a paper across all
-- versions (the page coords are version-scoped inside the ANCHOR JSON, not here).
-- PROJECT_FK is optional: when present, the annotation is scoped to a project
-- rather than the whole library (mirrors NOTE, and feeds the share snapshot).
-- ANCHOR is opaque JSON ({v,version,page,color,quote,rects}); validate_anchor
-- size-caps it, the frontend renderer reads its structural shape.
-- Added after the initial schema, so it is created by an idempotent startup
-- migration (CREATE TABLE IF NOT EXISTS) rather than the base TABLE_DDL.
CREATE TABLE IF NOT EXISTS ANNOTATION(
    ANNOTATION_SK INTEGER NOT NULL,
    SOURCE_FK     INTEGER NOT NULL,
    PROJECT_FK    INTEGER,
    ANCHOR        TEXT NOT NULL,
    COMMENT       TEXT NOT NULL DEFAULT '',
    CREATED_AT    TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    UPDATED_AT    TIMESTAMP NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (ANNOTATION_SK),
    FOREIGN KEY (SOURCE_FK)  REFERENCES PAPER_ROOTS(SOURCE_FK) ON DELETE CASCADE,
    FOREIGN KEY (PROJECT_FK) REFERENCES PROJECT(PROJECT_FK)
);
CREATE INDEX IF NOT EXISTS idx_annotation_source_fk ON ANNOTATION (SOURCE_FK);
CREATE INDEX IF NOT EXISTS idx_annotation_project_fk ON ANNOTATION (PROJECT_FK);

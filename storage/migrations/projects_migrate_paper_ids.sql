-- Migrate paper_ids JSON array column into project_papers bridge table.
-- json_each key is the 0-based array index, used as position.
INSERT OR IGNORE INTO project_papers (project_id, paper_id, position)
SELECT p.id, je.value, CAST(je.key AS INTEGER)
FROM projects p, json_each(p.paper_ids) AS je
WHERE p.paper_ids IS NOT NULL;

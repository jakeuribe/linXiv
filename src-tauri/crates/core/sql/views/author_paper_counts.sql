DROP VIEW IF EXISTS author_paper_counts;

-- One row per AUTHOR_FK with the number of distinct *active* papers (roots)
-- the author is linked to. Authors with no active papers report 0.
-- Used to drive the "exclude single-paper authors" option on the Authors
-- page and the graph, and computed in SQL so the filter scales as the
-- library grows.
CREATE VIEW author_paper_counts AS
SELECT
    a.AUTHOR_FK AS author_fk,
    COUNT(DISTINCT CASE WHEN pr.STATUS = 'active' THEN p.SOURCE_FK END) AS paper_count
FROM AUTHOR a
LEFT JOIN PAPER_TO_AUTHOR pta ON pta.AUTHOR_FK = a.AUTHOR_FK
LEFT JOIN PAPER p             ON p.PAPER_ID    = pta.PAPER_ID
LEFT JOIN PAPER_ROOTS pr      ON pr.SOURCE_FK  = p.SOURCE_FK
GROUP BY a.AUTHOR_FK;

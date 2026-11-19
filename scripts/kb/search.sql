-- Parameters are the escaped FTS expression, optional PowerShell version, exact first term and result limit.
WITH hits AS MATERIALIZED (
 SELECT rowid,rank FROM search
 WHERE search MATCH ?1 AND rank MATCH 'bm25(20.0,6.0,1.0)'
 AND (?2 IS NULL OR EXISTS(SELECT 1 FROM chunks c WHERE c.id=search.rowid AND c.ps=?2))
 ORDER BY rank LIMIT 100
), candidates AS (
 SELECT c.id,c.command,c.heading,substr(c.text,1,1200) AS excerpt,c.status,c.ps,c.applies,c.risk,
 d.repo,d.path,s.sha,h.rank,row_number() OVER (
 PARTITION BY lower(CASE WHEN c.command='' THEN d.path ELSE c.command END)
 ORDER BY h.rank,c.id) AS occurrence
 FROM hits h JOIN chunks c ON c.id=h.rowid JOIN documents d ON d.id=c.document JOIN sources s ON s.repo=d.repo
)
SELECT command,heading,excerpt,status,ps,applies,risk,repo,path,sha,id FROM candidates WHERE occurrence=1
ORDER BY command=?3 COLLATE NOCASE DESC,rank,id LIMIT ?4

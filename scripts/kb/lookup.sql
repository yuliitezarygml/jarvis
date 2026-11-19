SELECT c.command,c.heading,substr(c.text,1,1200),c.status,c.ps,c.applies,c.risk,d.repo,d.path,s.sha,c.id
FROM chunks c JOIN documents d ON d.id=c.document JOIN sources s ON s.repo=d.repo
WHERE c.command=?1 COLLATE NOCASE
ORDER BY CASE WHEN upper(c.heading) LIKE '%SYNOPSIS' THEN 0 WHEN upper(c.heading) LIKE '%SYNTAX%' THEN 2 ELSE 1 END,c.id LIMIT 5

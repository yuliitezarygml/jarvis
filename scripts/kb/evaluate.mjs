import { readFileSync } from 'node:fs';
const read = name => readFileSync(new URL(name,import.meta.url),'utf8');
export function evaluate(db) {
  const stmt = db.prepare(read('search.sql'));
  const times = [], misses = [];
  const cases = read('questions.txt').trim().split(/\r?\n/).filter(l=>!l.startsWith('#'));
  if (new Set(cases.map(line=>line.split('|')[0])).size < 100) throw Error('Expected at least 100 distinct questions');
  for (const line of cases) {
    const [question,query,expected] = line.split('|');
    const expr = query.match(/[A-Za-z0-9]+/g).map(t=>'"'+t+'"*').join(' AND ');
    const start = performance.now();
    const hits = stmt.all({1:expr,2:null,3:query.split(/\s+/)[0],4:5});
    times.push(performance.now()-start);
    if (!hits.some(row=>row.command.toLowerCase()===expected.toLowerCase())) misses.push(question);
  }
  times.sort((a,b)=>a-b);
  return {questions:cases.length,hits:cases.length-misses.length,recall_at_5:(cases.length-misses.length)/cases.length,
    p50_ms:+times[Math.floor(times.length*.5)].toFixed(2),p95_ms:+times[Math.floor(times.length*.95)].toFixed(2),misses};
}

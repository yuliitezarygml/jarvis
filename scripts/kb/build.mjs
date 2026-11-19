import { DatabaseSync } from 'node:sqlite';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream, existsSync, mkdirSync, readFileSync, writeFileSync, readdirSync, statSync, copyFileSync, renameSync, rmSync, mkdtempSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { parseMarkdown, applicability } from './markdown.mjs';
import { evaluate } from './evaluate.mjs';

const sql = name => readFileSync(new URL(name, import.meta.url), 'utf8');
// Bump when parsing or metadata rules change. A previous file hash is reusable
// only if the same indexer produced its rows.
const indexerVersion = '2';
function licenseRecord(file,text) {
  const id = /Attribution 4\.0 International|creativecommons\.org\/licenses\/by\/4\.0/i.test(text)
    ? 'CC-BY-4.0' : /MIT License|Permission is hereby granted, free of charge/i.test(text) ? 'MIT' : 'see license text';
  return {file,id,text};
}
function riskCeiling(command,heading,text) {
  if (/\b(?:remove|clear|stop|start|set|install|disable|enable|restart)-\w+|\/(?:release|renew|flushdns|registerdns)\b/i.test(text)) return 3;
  if (/^Get-[A-Za-z0-9]+$/i.test(command) && /\/ (SYNOPSIS|SYNTAX)\b/i.test(heading)) return 0;
  if (/^(?:hostname|whoami|systeminfo|getmac|tasklist|driverquery|ver|vol)$/i.test(command)) return 0;
  return 3;
}
const root = resolve(process.argv[2] || '.kb/sources');
const output = resolve(process.argv[3] || '.kb/local');
const catalog = JSON.parse(readFileSync(join(root,'sources.json'),'utf8'));
const missing = catalog.filter(s => s.error).map(s => s.repo);
if (!catalog.some(s => !s.error) || (missing.length && !process.argv.includes('--allow-partial')))
  throw Error('Unavailable sources: ' + missing.join(', ') + '. Use --allow-partial for a local build.');
mkdirSync(output,{recursive:true});
const started = performance.now(), stage = mkdtempSync(join(output,'.building-'));
const dbName = 'windows_kb.sqlite3', target = join(stage,dbName);
const marker = join(output,'current.json');
const lock = join(output,'.build-lock');
try { mkdirSync(lock); } catch { rmSync(stage,{recursive:true}); throw Error('Another build holds ' + lock); }
let db;
try {
  const previous = existsSync(marker) && JSON.parse(readFileSync(marker,'utf8')).version;
  if (previous) {
    const oldFile=join(output,previous,dbName);
    const old=new DatabaseSync(oldFile,{readOnly:true});
    let compatible;
    try { compatible=old.prepare("SELECT value FROM metadata WHERE key='indexer'").get()?.value===indexerVersion; }
    finally {old.close();}
    if (compatible) copyFileSync(oldFile,target);
  }
  const fresh = !existsSync(target);
  db = new DatabaseSync(target);
  db.exec('PRAGMA foreign_keys=ON; PRAGMA trusted_schema=ON;');
  if (fresh) db.exec(sql('schema.sql'));
  db.exec('BEGIN; CREATE TEMP TABLE seen(id INTEGER PRIMARY KEY);');
  const insert = db.prepare('INSERT INTO chunks(document,command,heading,text,module,applies,ps,admin,risk,status,date) VALUES(?,?,?,?,?,?,?,?,?,?,?)');
  const doc = db.prepare('SELECT id,hash FROM documents WHERE repo=? AND path=?');
  const seen = db.prepare('INSERT INTO seen VALUES(?)');
  const del = db.prepare('DELETE FROM documents WHERE id=?');
  const add = db.prepare('INSERT INTO documents(repo,path,hash,title,description,front) VALUES(?,?,?,?,?,?)');
  let changed = 0;
  const notices = [], stats = [];
  for (const s of catalog.filter(s=>!s.error)) {
    const dir = join(root,basename(s.repo));
    const actual = execFileSync('git',['-C',dir,'rev-parse','HEAD'],{encoding:'utf8'}).trim();
    if (actual !== s.sha || execFileSync('git',['-C',dir,'status','--porcelain'],{encoding:'utf8'}).trim()) throw Error('Source cache changed: ' + s.repo);
    const licenses = s.licenses.map(file=>licenseRecord(file,readFileSync(join(dir,file),'utf8')));
    db.prepare('INSERT INTO sources VALUES(?,?,?,?,?) ON CONFLICT(repo) DO UPDATE SET url=excluded.url,branch=excluded.branch,sha=excluded.sha,licenses=excluded.licenses')
      .run(s.repo,s.url,s.branch,s.sha,JSON.stringify(licenses));
    notices.push(s.repo+' · '+s.url+' · '+s.sha,'Index of excerpts from the original authors and repository contributors. Section headings and provenance retained.', ...licenses.map(l=>l.file+' ('+l.id+')\n'+l.text));
    stats.push({repo:s.repo,url:s.url,branch:s.branch,sha:s.sha,licenses:licenses.map(({file,id})=>({file,id})),sections:s.dirs});
    for (const section of s.dirs) {
      const base = join(dir,section);
      for (const item of readdirSync(base,{recursive:true,withFileTypes:true})) {
        if (!item.isFile() || !item.name.endsWith('.md') || /^(?:toc|readme|index)\.md$/i.test(item.name)) continue;
        const file = join(item.parentPath,item.name), path = file.slice(dir.length+1).replaceAll('\\','/');
        const raw = readFileSync(file), hash = createHash('sha256').update(raw).digest('hex');
        const old = doc.get(s.repo,path);
        if (old?.hash === hash) {seen.run(old.id); continue}
        if (old) del.run(old.id);
        const {title, meta, front, sections} = parseMarkdown(raw.toString('utf8'));
        const stem = basename(file,'.md');
        const id = add.run(s.repo,path,hash,title || stem,meta.description || '',front).lastInsertRowid;
        seen.run(id); changed++;
        const cmd = ['commands','tldr'].includes(s.kind) || /^(?:[A-Za-z]+-[\w-]+|about_[\w-]+)$/.test(stem) ? stem : '';
        const ps = /^reference\/([\d.]+)\//.exec(path)?.[1] || 'unknown';
        const moduleName = meta['module name'] || (s.kind === 'modules' ? basename(dirname(file)) : 'unknown');
        const applies = applicability(s.kind,front,sections);
        for (const part of sections) {
          const admin = /\brequires? (?:administrative privileges|administrator privileges|an elevated (?:command prompt|session))\b/i.test(part.text) ? 1 : null;
          // Conservative upper bound for unreviewed commands. This field never authorizes execution.
          insert.run(id,cmd,part.heading,part.text,moduleName,applies,ps,admin,riskCeiling(cmd,part.heading,part.text),'doc_derived',meta['ms.date'] || null);
        }
      }
    }
  }
  db.exec('DELETE FROM documents WHERE id NOT IN (SELECT id FROM seen); DELETE FROM sources WHERE repo NOT IN (SELECT repo FROM documents); COMMIT;');
  const counts = db.prepare("SELECT (SELECT count(*) FROM documents) documents,count(*) chunks,avg(status='vm_verified') vm_verified FROM chunks").get();
  if (!counts.chunks) throw Error('Empty snapshot');
  db.exec("INSERT INTO search(search,rank) VALUES('integrity-check',1)");
  const version = new Date().toISOString().replace(/[:.]/g,'-')+'-'+randomUUID().slice(0,8);
  const manifest = {version,date:new Date().toISOString(),signature:'local_unsigned',sources:stats,missing,...counts};
  db.prepare("INSERT OR REPLACE INTO metadata VALUES('manifest',?)").run(JSON.stringify(manifest));
  db.prepare("INSERT OR REPLACE INTO metadata VALUES('indexer',?)").run(indexerVersion);
  const metrics = evaluate(db);
  if (metrics.recall_at_5 < 0.70 || db.prepare('PRAGMA quick_check').get().quick_check !== 'ok') throw Error('Snapshot validation failed: '+JSON.stringify(metrics));
  db.close(); db=null;
  const hash = createHash('sha256');
  for await (const block of createReadStream(target)) hash.update(block);
  Object.assign(manifest,{sha256:hash.digest('hex'),size:statSync(target).size,changed,metrics,build_ms:Math.round(performance.now()-started)});
  writeFileSync(join(stage,'manifest.json'),JSON.stringify(manifest,null,2));
  writeFileSync(join(stage,'NOTICE'),notices.join('\n\n'));
  renameSync(stage,join(output,version));
  writeFileSync(marker+'.tmp',JSON.stringify({version}));
  renameSync(marker+'.tmp',marker);
  console.log(JSON.stringify({version,...counts,changed,metrics,bytes:manifest.size,ms:manifest.build_ms}));
  const snapshots = readdirSync(output,{withFileTypes:true}).filter(d=>d.isDirectory() && /^\d{4}-\d\d-\d\dT[\dZ-]+-[0-9a-f]{8}$/.test(d.name)).map(d=>d.name).sort().reverse();
  for (const expired of snapshots.slice(3)) rmSync(join(output,expired),{recursive:true});
} finally {
  db?.close();
  rmSync(stage,{recursive:true,force:true});
  rmSync(lock,{recursive:true});
}

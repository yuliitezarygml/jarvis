# jarvis-kb

This crate reads the Windows knowledge base `data/kb.sqlite`. The database is built by `tools/kb-build`.

## Schema
- `chunks`: one row per fragment of a document. Fragments are split by H2/H3 at about 700 tokens.
- `chunks_fts`: an FTS5 table with external content. The column order is `title, commands, heading_path, body`. It ranks with `bm25(5, 4, 3, 1)` and uses `tokenize='unicode61 remove_diacritics 2'` and `prefix='2 3 4'`.
- `commands`: exact command names mapped to fragments. `is_primary=1` marks the page of the command itself.
- `sources` and `meta`: the build report and the schema version.

## API
```rust
let kb = Kb::open("data/kb.sqlite")?;            // read-only; NotBuilt if the file is missing
kb.search(&["disable startup apps".into(), "Task Manager startup".into()], 6)?; // Vec<KbHit>
kb.lookup_command("robocopy")?;                   // exact command name, the command's own page comes first
kb.status()?;                                     // KbStatus { chunks, sources, built_at, .. }
kb.reload()?;                                     // reopen after kb-build (applies kb.sqlite.new)
let tools = jarvis_kb::tools(Arc::new(kb));       // kb_search, kb_lookup_command, kb_status (T0)
```
`KbHit` has the fields `title, url, heading, snippet, score, source, path`.

## Search
- Each query is stemmed with Snowball (English or Russian) and turned into prefix terms, for example `"disabl"*`.
- `Verb-Noun` cmdlets become phrases.
- Each query produces two lists, AND and OR. Commands found in the query add a list of exact matches.
- The lists are merged with RRF (k=20). The result keeps the best fragment of each document.
- The parameters are in `SearchParams` and can be tuned with `kb-build --eval --param ...`.

Tool output is marked `untrusted: true` because it is documentation text from outside the project.
If Jarvis holds the database open, kb-build writes `kb.sqlite.new`. `Kb::open` and `Kb::reload` pick it up.

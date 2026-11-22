# kb-build

`kb-build` builds the Windows knowledge base `data/kb.sqlite` from sparse clones of the MicrosoftDocs repositories (kept in `data/kb-src/`) and from `kb/seed/*.md`.

```
cargo run --release --manifest-path tools/kb-build/Cargo.toml --          # build
kb-build --update          # git fetch --depth 1 + reset for existing clones
kb-build --only seed       # rebuild one source and carry the others over from the old database
kb-build --offline         # no network access
kb-build --eval [--verbose] [--param rrf_k=30]   # Recall@5 and MRR@10 on eval/gold.jsonl
kb-build --check-urls 8    # HEAD-check 8 URLs per source
kb-build --search "query"  /  --show <part of URL>   # debugging
```

## How the build works
1. It clones each repository with `git clone --depth 1 --filter=blob:none --sparse`, then runs `git sparse-checkout set --cone`. Git runs with no credential prompts.
2. If the main repo address fails, it tries the mirrors. A source that is still unavailable produces a warning, and its old fragments are carried over.
3. For each file it reads the YAML front matter (title/description/ms.date, and skips `redirect_url` pages). It cleans the docfx markup: includes are inlined, `[!NOTE]` becomes «Примечание:», and `:::` lines are removed.
4. It parses the Markdown with `pulldown-cmark` and splits it into fragments by H2/H3 at about 700 tokens. A code block is never split.
5. It extracts commands: `Verb-Noun` cmdlets, and exe names from titles in `windows-commands` and from code blocks.
6. It builds the learn.microsoft.com URL using the rules in `kb_sources.json`.
7. It writes the database to a temporary file and renames it into place. The report goes to stdout and to `data/kb-build-report.json`.

## Sources (`kb_sources.json`), checked 2026-11-22
| id | repository / subpaths |
|---|---|
| windows-commands | MicrosoftDocs/windowsserverdocs: `WindowsServerDocs/administration/windows-commands` |
| powershell | MicrosoftDocs/PowerShell-Docs: `reference/7.6`, `reference/5.1` (only cmdlets that are not in 7.6), `docs-conceptual/{learn,samples,security,install,windows-powershell}` |
| windows-powershell | MicrosoftDocs/windows-powershell-docs: 35 client modules from `docset/winserver2025-ps` (Defender, NetSecurity, NetTCPIP, ScheduledTasks, Storage, Appx, PnpDevice…) |
| sysinternals | MicrosoftDocs/sysinternals: `sysinternals` |
| win32 | MicrosoftDocs/win32: `desktop-src/{WinAuto, shell, inputdev, Services, SysInfo, Debug, ProcThread, Power, EventLog}`. These match the agent's tools: UIA, SendInput, services, registry, error codes, processes, power, event log |
| windows-client | **MicrosoftDocs/windows-itpro-docs returns 404.** The content now lives in the private `MicrosoftDocs/windows-docs-pr` (see `original_content_git_url` on learn). Microsoft is retiring its public Learn repositories by the end of 2026. The build uses the last public snapshot `09768ace` (2025-08-08) from personal forks; several forks have the same hash |
| seed | `kb/seed/*.md` (20 cheat sheets) |

The eval set has 65 pairs; `--eval` exits with code 2 if Recall@5 is below 90 %.

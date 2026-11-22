# J.A.R.V.I.S.

[Русский](README.md) · **English** · [Română](README.ro.md)

A voice assistant for Windows 11 inspired by J.A.R.V.I.S. from *Iron Man*. Say "Jarvis, …" — it listens, thinks and answers with a live voice. It can control the PC with the mouse and keyboard (with an on-screen glow so you can see what it is doing), search the web, answer from the Windows documentation, remember you and learn from its own mistakes.

Stack: **Rust + Tauri 2 + Next.js**. No Python in the project. The detailed plan and decisions are in [docs/PLAN.md](docs/PLAN.md) (in Russian).

> Status: the modules are assembled into one application (shared Cargo workspace, orchestrator in `src-tauri/src/runtime`). The first live run with the microphone and PC control is next.

## Features

Legend: ✅ module done and tested · 🟡 partially done · ⏳ in progress. All modules have so far been tested separately; the combined build is part of the integration stage.

### 🎙️ Voice conversation — 🟡
- Wake phrase "**Jarvis, …**" — understands recognition variants ("джервис", "jarvis", etc.); without it, it does not react.
- Always listening; detects by itself where a phrase starts and ends (0.8–1.2 s pause).
- Russian speech recognition on its own Xeon server, with Groq Whisper in the cloud as a fallback. ⏳
- Answers with the live `jarvis-ru` voice and starts speaking right away without waiting for the full answer (first sound ~0.2–0.7 s).
- Reads correctly numbers with Russian case agreement ("2 файла", "5 файлов"), dates, times, percentages, currencies, units, abbreviations (Wi-Fi, VPN, CPU…), Markdown and tables; calls code "a code example".
- **Barge-in**: say "stop", "enough" or start a new phrase — it goes silent and cancels the answer. ⏳
- Does not mistake its own voice for a command (echo filter).
- Butler character from the film: addresses you as "sir", answers briefly, in Russian.

### 🧠 Brain and models — ✅
- Decides on its own which tools it needs and works in an **"observe → act → verify"** loop: checks the result after every step and tries another way on failure (up to 25 steps).
- Notices when it is going in circles and changes approach.
- Remembers the current conversation, so you can follow up: "now close it".
- **16 LLM providers**: Groq, OpenCode Zen, OpenRouter, OpenAI, Anthropic (Claude), Gemini, Mistral, DeepSeek, Together, Fireworks, Cerebras, xAI, Z.AI, local Ollama and LM Studio, any custom URL.
- **Add a key — get the best models**: validates the key, downloads the model list, tests the candidates itself (speed, Russian, tool calling, vision) and shows the top 3 with an explanation like "tools ✓ · Russian ✓ · 0.4 s · 128k". A "pick the best automatically" button.
- Separate models per role: main "brain", fast replies, vision (screenshots), self-learning.
- **Failover chain**: if a model does not respond, hits its limit or was removed, it automatically switches to the next one.

### 🖐️ Computer control — ✅
- **Mouse**: smooth movement (you can see where it goes), click, double and right click, drag, scroll; multiple monitors and display scaling.
- **Keyboard**: types any text, including Cyrillic; hotkeys ("Ctrl+Shift+S"); media keys.
- **Windows and buttons** via UI Automation: sees the list of a window's elements, presses buttons, fills in fields, toggles checkboxes, expands menus, switches between windows.
- **Vision**: screenshot of the screen or a window, numbered marks on elements for the vision model ("click [3]"), the OCR built into Windows (Russian and English, offline).
- **Glow** while Jarvis controls the PC: a glowing frame around the screen edge (blue — normal action, amber — dangerous, red — waiting for confirmation), a highlight of the target element labelled "Clicking: Save", a cursor trail, a "JARVIS IS CONTROLLING THE PC" badge.
- Jarvis's own windows never appear in screenshots — the model sees only your screen.
- **Ctrl+Alt+K** — instant emergency stop, all held keys are released.

### 🔎 Search and open — ⏳
- "Open Chrome", "open Notepad" — finds the program among open windows, the Start menu, the registry and PATH.
- "Open Bluetooth / Wi-Fi / sound / display settings…" — more than 30 Windows Settings pages, named in Russian.
- "Show the Downloads / Documents / Desktop folder".
- "Find the contract file on drive D and open it" — search by name, type, date and content; results are numbered: "open the second one", "show in Explorer".
- Clipboard, opening links.

### 💻 Terminal: "Jarvis, look at the error" — 🟡
- Reads the last command and error from your terminal: from its own built-in terminal, via a PowerShell hook, from the Windows Terminal window or from the screen (OCR).
- Explains the cause in 1–2 sentences ("typo: `comit` instead of `commit`", "no permissions", "module not installed").
- Shows the fixed command in large type in the HUD and **types it into the terminal itself**; dangerous ones only after "yes, sir". A "just tell me" mode — without typing.
- Checks that the fix worked, otherwise tries another way (up to 3 attempts).
- Corrects you: briefly says what was wrong with your command.
- **Remembers the fix**: the same error next time is fixed instantly, without calling the model or the internet. ⏳

### ⚙️ System and resources — ⏳
- Runs PowerShell and cmd: shows output and exit code, kills hung commands together with their child processes.
- File operations: read, write, copy, move, delete.
- Processes: list, "what is eating memory right now?", close, suspend, priority.
- Services, startup apps, power plans, Task Scheduler, network (ping, DNS flush), installing programs via winget.
- Telemetry in the HUD: CPU, memory, GPU, disks, network.
- **Resource guard**: warns you by itself — "Sir, memory is 93 % full, Chrome is using the most" — and suggests what to do.
- **Undo**: deletion goes to the Recycle Bin, a copy is saved before overwriting, the registry is exported before editing; "undo the last action".

### 🛡️ Security — 🟡
- Four risk levels for every action:

  | Level | Examples | Behaviour |
  |---|---|---|
  | T0 read | processes, services, screen, search | on its own, silently |
  | T1 action | clicks, typing, launching programs, volume | on its own, with glow |
  | T2 profile changes | files in your folders, user settings | on its own, with glow and undo |
  | T3 dangerous | deleting outside working folders, HKLM registry, services, installing programs, firewall, shutting down the PC | says what it will do and waits for "yes, sir" (20 s) |

- An unknown command is treated as dangerous.
- System folders (`C:\Windows`, `Program Files`) require a second confirmation.
- "Yes" is accepted only from the microphone and only after Jarvis has finished speaking, so it cannot confirm itself.
- Text from websites, the terminal and the screen is **data, not commands**: a page saying "delete all files" will not run anything.
- Keys and passwords are stripped from text before it is sent to the cloud; keys are masked in the interface.
- The web module does not access the local network (router, server), so a malicious page cannot reach it through Jarvis.
- Runs as administrator (one UAC prompt at startup), so it can manage services, the registry and windows of other programs running as administrator.

### 🌐 Web — ✅
- "Google…": search via DuckDuckGo without a key; with keys — Brave, Tavily, Serper, Jina (more accurate). A key is validated with the "Check" button.
- Reads web pages and PDFs, strips ads and menus, keeps the essence.
- **Research with sources**: writes the queries itself, reads 3–5 sites, answers by voice in 2–4 sentences and shows the links in the HUD.
- "Open the second source", "read it in more detail".

### 📚 Windows knowledge base — ✅
- ~20k chunks of Microsoft documentation: every Windows command, PowerShell (including Defender, firewall, Task Scheduler, network, disks), Sysinternals, the relevant parts of the Win32 API, IT Pro docs, plus 20 own cheat sheets.
- Finds the right document in the top five **95.4 %** of the time, 9 out of 9 for Russian questions; answers with a link to learn.microsoft.com.
- Works offline.

### 🧩 Memory and self-learning — ⏳
- **Facts about you**: "remember that I drink black coffee" — remembers it even after a restart.
- **Lessons from mistakes**: what went wrong, why and how to fix it; before a similar action it warns itself "last time…".
- **Playbooks**: successful sequences of actions are saved as recipes and reused.
- **Self-learning**: after every task it reviews what happened and writes the conclusions into memory itself; takes your corrections into account ("no, not like that").
- Understands Russian morphology: "какой кофе я любил" finds "я люблю чёрный кофе".
- Cleans memory once a week: merges duplicates, forgets what is unused.
- All memory can be viewed, edited and deleted in the interface.
- This is experience accumulated in memory, not fine-tuning of the model itself.

### 🖥️ Interface like in the film — ✅ (not yet checked visually)
- A J.A.R.V.I.S.-style **reactor**: rotating rings, blue glow, a voice wave; changes with the state — idle ("breathing"), listening, thinking (scanning arc), speaking (pulsing with speech), controlling the PC (amber accents), error (red flicker).
- A **corner widget** (220 px, always on top, draggable) and a **fullscreen HUD** with telemetry, an action feed, subtitles and sources; switch with a click, by voice, with **Ctrl+Alt+J** or from the tray.
- **Action feed**: what Jarvis is doing now and what it has already done.
- A **confirmation dialog** with "Yes / No" buttons and a timer.
- **Settings**: providers and models, voice and servers, audio, permissions, windows, search.
- "Memory", "Knowledge base" and "Log" screens.
- Demo simulator: cycles through all states without needing the backend.

## How it works

```mermaid
flowchart LR
    subgraph EARS["🎙️ Hearing"]
        MIC[Microphone] --> VAD[VAD<br/>start and end of phrase]
        VAD --> STT[STT<br/>Xeon server]
        STT --> WAKE{"Jarvis, …"}
    end

    subgraph BRAIN["🧠 Brain"]
        MEM[(Memory<br/>facts · lessons · playbooks)]
        KB[(Knowledge base<br/>Windows)]
        AGENT[Agent<br/>LLM provider chain]
        POLICY{Risk level<br/>T0 – T3}
        MEM --> AGENT
        KB --> AGENT
        AGENT --> POLICY
    end

    subgraph HANDS["🖐️ Hands"]
        PC[Mouse · keyboard<br/>windows · screen · OCR]
        SYS[Terminal · PowerShell<br/>files · resources]
        WEB[Web<br/>search · pages]
    end

    subgraph VOICE["🔊 Voice"]
        NORM[Speech normalizer] --> TTS[TTS<br/>Xeon server] --> SPK[Speakers]
    end

    subgraph UI["🖥️ Interface"]
        HUD[Fullscreen HUD]
        CORNER[Corner reactor widget]
        GLOW[Glow overlay]
        SET[Settings]
    end

    WAKE --> AGENT
    POLICY -- "T0–T2: on its own" --> PC & SYS & WEB
    POLICY -- "T3: «yes, sir»?" --> CONFIRM([Confirmation]) --> SYS
    PC & SYS & WEB -- result --> AGENT
    AGENT -- answer --> NORM
    AGENT -. events .-> UI
    AGENT -. lessons .-> MEM
```

All modules are built on the shared crate [`crates/jarvis-core`](crates/jarvis-core): event, tool, LLM and config types, plus the STT protocol.

## Modules and status

| Module | Folder | What it does | Status | Tests |
|---|---|---|---|---|
| Core | [crates/jarvis-core](crates/jarvis-core) | shared contracts | ✅ | 14 |
| Brain | [crates/jarvis-agent](crates/jarvis-agent) | "observe → act → verify" loop, confirmations, glow | ✅ | 46 |
| LLM providers | [crates/jarvis-llm](crates/jarvis-llm) | 16 providers (Groq, OpenCode Zen, OpenRouter, OpenAI, Anthropic, Gemini…), key validation, best-model selection, failover chain | ✅ | 69 (+3 live) |
| Web | [crates/jarvis-web](crates/jarvis-web) | search (DuckDuckGo without a key, Brave, Tavily…), page reading, research with sources, protection from malicious "instructions" | ✅ | 64 (+2 live) |
| PC | [crates/jarvis-pc](crates/jarvis-pc) | mouse, keyboard, UI Automation, screenshots with marks, OCR, terminal | ✅ | 83 (+8 live input) |
| Knowledge base | [crates/jarvis-kb](crates/jarvis-kb), [tools/kb-build](tools/kb-build), [kb/seed](kb/seed) | ~20k chunks of Microsoft documentation + cheat sheets; Recall@5 = 95.4 % | ✅ | 72 |
| Interface | [src](src), [src-tauri](src-tauri) | HUD, corner reactor widget, glow overlay, settings, demo simulator | ✅ (not yet tested live) | 39 + 25 |
| Voice | [crates/jarvis-voice](crates/jarvis-voice) | TTS client, speech normalizer, VAD, "Jarvis / stop / yes", echo filter, STT clients | 🟡 no speakers, microphone or pipeline yet | 54 (+3 live) |
| Memory | [crates/jarvis-memory](crates/jarvis-memory) | facts, lessons from mistakes, playbooks, self-learning | ⏸ in progress | — |
| System | [crates/jarvis-sys](crates/jarvis-sys) | command risk levels, PowerShell, undo, resources, search and open | ⏸ in progress | — |
| STT server | [server/jarvis-stt-rs](server/jarvis-stt-rs) | Russian speech recognition on the Xeon | ⏸ in progress | — |
| TTS server | [server/jarvis-tts-rs](server/jarvis-tts-rs) | `jarvis-ru` voice synthesis (running on the Xeon) | ✅ running | — |

"Live" tests go to the network or move the mouse and are skipped by default (`#[ignore]`).

## Voice server (Xeon 192.168.50.38)

- **TTS**: `https://192.168.50.38:8443`, no token, TLS with a private CA ([src-tauri/config/xeon-tts-ca.pem](src-tauri/config/xeon-tts-ca.pem)). `GET /ready`, `POST /tts/stream` → 24 kHz PCM. The model is downloaded from the server into `server/jarvis-tts-rs/model/` (not stored in git).
- **STT**: will be at `https://192.168.50.38:8444`; protocol — [crates/jarvis-core/src/stt_protocol.rs](crates/jarvis-core/src/stt_protocol.rs).

## Configuration

Everything lives in a single `config.json` in the project root (in `.gitignore`, never committed). A sample without secrets is [config.example.json](config.example.json):

```powershell
copy config.example.json config.json
```

It holds the LLM providers and keys, model chains per role (`brain`, `fast`, `vision`, `reflection`), search, TTS/STT addresses, permissions (`autonomy`: `ask_dangerous` — ask only about dangerous actions) and the window mode.

Jarvis's data (memory, knowledge base, undo journal) lives in `data/` (also outside git).

## Running and testing

Requirements: Rust (stable), Node.js 24, WebView2, `cargo install tauri-cli`.

```powershell
# interface with the demo simulator (backend not wired yet)
npm install
cargo tauri dev

# module tests (each crate builds separately)
cd crates\jarvis-agent; cargo test
npm test                     # frontend tests

# Windows knowledge base
cd tools\kb-build; cargo run --release -- --offline   # rebuild from what is already downloaded
cargo run --release -- --eval                          # measure accuracy
```

Hotkeys: **Ctrl+Alt+J** — fullscreen HUD / corner, **Ctrl+Alt+K** — emergency stop.

## Security

- Actions are split into levels: **T0** read, **T1** action (with glow), **T2** changes in the user profile (with undo), **T3** dangerous — Jarvis says what it is about to do and waits for "yes, sir". An unknown command is treated as dangerous.
- Text from websites, the terminal and the screen is data, not commands: Jarvis does not follow "instructions" found there.
- Keys and passwords are stripped from text before it is sent to the cloud.
- Keys are stored only in `config.json` and are masked in the interface and logs.

## Next steps

1. Finish Memory, System, Voice and the STT server.
2. Integration: wire the real modules in place of the interface's demo data, build a shared Cargo workspace.
3. End-to-end tests against the acceptance criteria in [docs/PLAN.md](docs/PLAN.md), first live run.

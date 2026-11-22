# J.A.R.V.I.S.

[Русский](README.md) · [English](README.en.md) · **Română**

Un asistent vocal pentru Windows 11 inspirat de J.A.R.V.I.S. din *Iron Man*. Spui „Jarvis, …” — el ascultă, gândește și răspunde cu voce vie. Poate controla PC-ul cu mouse-ul și tastatura (cu o strălucire pe ecran, ca să vezi ce face), caută pe internet, răspunde din documentația Windows, ține minte cine ești și învață din propriile greșeli.

Stack: **Rust + Tauri 2 + Next.js**. Fără Python în proiect. Planul detaliat și deciziile sunt în [docs/PLAN.md](docs/PLAN.md) (în rusă).

> Stare: modulele sunt asamblate într-o singură aplicație (workspace Cargo comun, orchestratorul în `src-tauri/src/runtime`). Urmează prima rulare reală cu microfonul și controlul PC-ului.

## Funcționalități

Legendă: ✅ modul gata și testat · 🟡 gata parțial · ⏳ în lucru. Deocamdată toate modulele au fost testate separat; asamblarea comună face parte din etapa de integrare.

### 🎙️ Conversație vocală — 🟡
- Adresarea „**Jarvis, …**” — înțelege variantele de recunoaștere („джервис”, „jarvis” etc.); fără ea nu reacționează.
- Ascultă permanent; își dă seama singur unde începe și unde se termină fraza (pauză de 0,8–1,2 s).
- Recunoașterea vorbirii în rusă pe propriul server Xeon, iar ca rezervă — Groq Whisper în cloud. ⏳
- Răspunde cu vocea vie `jarvis-ru` și începe să vorbească imediat, fără să aștepte tot răspunsul (primul sunet ~0,2–0,7 s).
- Citește corect numerele cu acordul în rusă („2 файла”, „5 файлов”), datele, ora, procentele, valutele, unitățile, abrevierile (Wi-Fi, VPN, CPU…), Markdown și tabelele; codul îl numește „exemplu de cod”.
- **Întrerupere**: spune „stop”, „ajunge” sau începe o frază nouă — tace și anulează răspunsul. ⏳
- Nu își confundă propria voce cu o comandă (filtru de ecou).
- Caracter de majordom ca în film: ți se adresează cu „domnule”, răspunde scurt, în rusă.

### 🧠 Creierul și modelele — ✅
- Decide singur ce instrumente îi trebuie și lucrează în ciclul **„observă → acționează → verifică”**: după fiecare pas verifică rezultatul și, la eșec, încearcă altfel (până la 25 de pași).
- Observă când se învârte în cerc și schimbă abordarea.
- Ține minte conversația curentă, deci poți continua: „acum închide-l”.
- **16 furnizori LLM**: Groq, OpenCode Zen, OpenRouter, OpenAI, Anthropic (Claude), Gemini, Mistral, DeepSeek, Together, Fireworks, Cerebras, xAI, Z.AI, Ollama și LM Studio locale, orice adresă proprie.
- **Adaugi o cheie — primești cele mai bune modele**: verifică cheia, descarcă lista de modele, testează singur candidații (viteză, limba rusă, apelarea instrumentelor, vedere) și arată top 3 cu o explicație de tipul „tools ✓ · rusă ✓ · 0,4 s · 128k”. Buton „alege automat cel mai bun”.
- Modele separate pe roluri: „creierul” principal, răspunsuri rapide, vedere (capturi de ecran), auto-învățare.
- **Lanț de rezervă**: dacă un model nu răspunde, și-a epuizat limita sau a fost eliminat, trece automat la următorul.

### 🖐️ Controlul calculatorului — ✅
- **Mouse**: mișcare lină (se vede încotro merge), clic, dublu clic și clic dreapta, tragere, derulare; mai multe monitoare și scalarea ecranului.
- **Tastatură**: introduce orice text, inclusiv în chirilică; taste rapide („Ctrl+Shift+S”); taste media.
- **Ferestre și butoane** prin UI Automation: vede lista elementelor unei ferestre, apasă butoane, completează câmpuri, bifează casete, deschide meniuri, comută între ferestre.
- **Vedere**: captura ecranului sau a unei ferestre, marcaje numerotate pe elemente pentru modelul de vedere („apasă [3]”), OCR-ul integrat în Windows (rusă și engleză, fără internet).
- **Strălucire** cât timp Jarvis controlează PC-ul: un cadru luminos pe marginea ecranului (albastru — acțiune obișnuită, chihlimbar — periculoasă, roșu — așteaptă confirmare), evidențierea elementului-țintă cu eticheta „Apăs: Salvează”, urma cursorului, insigna „JARVIS CONTROLEAZĂ PC-UL”.
- Ferestrele lui Jarvis nu apar în capturile de ecran — modelul vede doar ecranul tău.
- **Ctrl+Alt+K** — oprire de urgență instantanee, toate tastele apăsate sunt eliberate.

### 🔎 Căutare și deschidere — ⏳
- „Deschide Chrome”, „deschide Notepad” — găsește programul printre ferestrele deschise, meniul Start, registru și PATH.
- „Deschide setările Bluetooth / Wi-Fi / sunet / afișaj…” — peste 30 de pagini din Setările Windows, denumite în rusă.
- „Arată folderul Descărcări / Documente / Desktop”.
- „Găsește fișierul contract pe discul D și deschide-l” — căutare după nume, tip, dată și conținut; rezultatele sunt numerotate: „deschide-l pe al doilea”, „arată în Explorer”.
- Clipboard, deschiderea linkurilor.

### 💻 Terminal: „Jarvis, uită-te la eroare” — 🟡
- Citește ultima comandă și eroarea din terminalul tău: din propriul terminal integrat, printr-un hook PowerShell, din fereastra Windows Terminal sau de pe ecran (OCR).
- Explică cauza în 1–2 fraze („greșeală de tipar: `comit` în loc de `commit`”, „lipsesc drepturile”, „modulul nu este instalat”).
- Arată comanda corectată cu litere mari în HUD și **o introduce singur** în terminal; pe cele periculoase — doar după „da, domnule”. Modul „doar spune-mi” — fără introducere.
- Verifică dacă după corectare totul a mers, altfel încearcă altă cale (până la 3 încercări).
- Te corectează: spune pe scurt ce a fost greșit în comanda ta.
- **Ține minte soluția**: aceeași eroare data viitoare se corectează instantaneu, fără model și fără internet. ⏳

### ⚙️ Sistem și resurse — ⏳
- Execută PowerShell și cmd: arată ieșirea și codul de eroare, oprește comenzile blocate împreună cu procesele copil.
- Lucrul cu fișiere: citire, scriere, copiere, mutare, ștergere.
- Procese: listă, „ce consumă acum memoria?”, închidere, suspendare, prioritate.
- Servicii, pornire automată, planuri de alimentare, Task Scheduler, rețea (ping, golirea DNS), instalarea programelor prin winget.
- Telemetrie în HUD: procesor, memorie, placă video, discuri, rețea.
- **Paznicul resurselor**: te avertizează singur — „Domnule, memoria e plină în proporție de 93 %, Chrome consumă cel mai mult” — și propune ce să faci.
- **Anulare**: ștergerea merge în Coșul de reciclare, înainte de suprascriere se păstrează o copie, înainte de modificarea registrului se face un export; „anulează ultima acțiune”.

### 🛡️ Securitate — 🟡
- Patru niveluri de risc pentru fiecare acțiune:

  | Nivel | Exemple | Cum acționează |
  |---|---|---|
  | T0 citire | procese, servicii, ecran, căutare | singur, în tăcere |
  | T1 acțiune | clicuri, tastare, pornirea programelor, volum | singur, cu strălucire |
  | T2 modificări în profil | fișiere din folderele tale, setările utilizatorului | singur, cu strălucire și anulare |
  | T3 periculos | ștergere în afara folderelor de lucru, registrul HKLM, servicii, instalarea programelor, firewall, oprirea PC-ului | spune ce va face și așteaptă „da, domnule” (20 s) |

- O comandă necunoscută este considerată periculoasă.
- Folderele de sistem (`C:\Windows`, `Program Files`) cer o a doua confirmare.
- „Da” este acceptat doar de la microfon și doar după ce Jarvis a terminat de vorbit, deci nu se poate confirma singur.
- Textul de pe site-uri, din terminal și de pe ecran este **dată, nu comandă**: o pagină cu „șterge toate fișierele” nu va porni nimic.
- Cheile și parolele sunt eliminate din text înainte de a fi trimis în cloud; în interfață cheile sunt mascate.
- Modulul de internet nu accesează rețeaua locală (router, server), ca o pagină străină să nu poată ajunge acolo prin Jarvis.
- Rulează ca administrator (o singură cerere UAC la pornire), deci poate gestiona servicii, registrul și ferestrele altor programe pornite ca administrator.

### 🌐 Internet — ✅
- „Caută pe Google…”: căutare prin DuckDuckGo fără cheie; cu chei — Brave, Tavily, Serper, Jina (mai precis). Cheia se verifică cu butonul „Verifică”.
- Citește pagini și PDF-uri, elimină reclamele și meniurile, păstrează esențialul.
- **Cercetare cu surse**: își formulează singur interogările, citește 3–5 site-uri, răspunde vocal în 2–4 fraze și arată linkurile în HUD.
- „Deschide a doua sursă”, „citește mai detaliat”.

### 📚 Baza de cunoștințe Windows — ✅
- ~20 de mii de fragmente din documentația Microsoft: toate comenzile Windows, PowerShell (inclusiv Defender, firewall, Task Scheduler, rețea, discuri), Sysinternals, părțile necesare din Win32 API, documentația pentru IT Pro, plus 20 de fișe proprii.
- Găsește documentul potrivit în primele cinci rezultate în **95,4 %** din cazuri, iar la întrebările în rusă — 9 din 9; răspunde cu link spre learn.microsoft.com.
- Funcționează fără internet.

### 🧩 Memorie și auto-învățare — ⏳
- **Fapte despre tine**: „ține minte că beau cafea neagră” — își amintește și după repornire.
- **Lecții din greșeli**: ce a mers prost, de ce și cum se corectează; înaintea unei acțiuni similare se avertizează singur „data trecută…”.
- **Playbook-uri**: secvențele reușite de acțiuni sunt salvate ca rețete și refolosite.
- **Auto-învățare**: după fiecare sarcină analizează ce s-a întâmplat și își scrie singur concluziile în memorie; ține cont de corecturile tale („nu, nu așa”).
- Înțelege morfologia rusă: „какой кофе я любил” găsește „я люблю чёрный кофе”.
- O dată pe săptămână își curăță memoria: unește duplicatele, uită ce nu se folosește.
- Toată memoria poate fi văzută, corectată și ștearsă în interfață.
- Este experiență acumulată în memorie, nu reantrenarea modelului însuși.

### 🖥️ Interfață ca în film — ✅ (neverificată vizual)
- **Reactor** în stilul J.A.R.V.I.S.: inele care se rotesc, strălucire albastră, undă de la voce; se schimbă după stare — așteaptă („respiră”), ascultă, gândește (arc de scanare), vorbește (pulsează în ritmul vorbirii), controlează PC-ul (accente chihlimbar), eroare (pâlpâire roșie).
- **Widget în colț** (220 px, deasupra tuturor ferestrelor, se poate trage) și **HUD pe tot ecranul** cu telemetrie, fluxul acțiunilor, subtitrări și surse; comutare cu clic, prin voce, cu **Ctrl+Alt+J** sau din tray.
- **Fluxul acțiunilor**: ce face Jarvis acum și ce a făcut deja.
- **Fereastră de confirmare** cu butoanele „Da / Nu” și cronometru.
- **Setări**: furnizori și modele, voce și servere, audio, permisiuni, ferestre, căutare.
- Ecranele „Memorie”, „Bază de cunoștințe”, „Jurnal”.
- Simulator demo: parcurge toate stările fără să fie nevoie de backend.

## Cum funcționează

```mermaid
flowchart LR
    subgraph EARS["🎙️ Auz"]
        MIC[Microfon] --> VAD[VAD<br/>începutul și sfârșitul frazei]
        VAD --> STT[STT<br/>server Xeon]
        STT --> WAKE{„Jarvis, …”}
    end

    subgraph BRAIN["🧠 Creier"]
        MEM[(Memorie<br/>fapte · lecții · playbook-uri)]
        KB[(Bază de cunoștințe<br/>Windows)]
        AGENT[Agent<br/>lanț de furnizori LLM]
        POLICY{Nivel de risc<br/>T0 – T3}
        MEM --> AGENT
        KB --> AGENT
        AGENT --> POLICY
    end

    subgraph HANDS["🖐️ Mâini"]
        PC[Mouse · tastatură<br/>ferestre · ecran · OCR]
        SYS[Terminal · PowerShell<br/>fișiere · resurse]
        WEB[Internet<br/>căutare · pagini]
    end

    subgraph VOICE["🔊 Voce"]
        NORM[Normalizator de vorbire] --> TTS[TTS<br/>server Xeon] --> SPK[Difuzoare]
    end

    subgraph UI["🖥️ Interfață"]
        HUD[HUD pe tot ecranul]
        CORNER[Widget-reactor în colț]
        GLOW[Overlay-strălucire]
        SET[Setări]
    end

    WAKE --> AGENT
    POLICY -- "T0–T2: singur" --> PC & SYS & WEB
    POLICY -- "T3: «da, domnule»?" --> CONFIRM([Confirmare]) --> SYS
    PC & SYS & WEB -- rezultat --> AGENT
    AGENT -- răspuns --> NORM
    AGENT -. evenimente .-> UI
    AGENT -. lecții .-> MEM
```

Toate modulele se bazează pe crate-ul comun [`crates/jarvis-core`](crates/jarvis-core): tipurile de evenimente, instrumente, LLM și configurație, plus protocolul STT.

## Module și stare

| Modul | Folder | Ce face | Stare | Teste |
|---|---|---|---|---|
| Nucleu | [crates/jarvis-core](crates/jarvis-core) | contracte comune | ✅ | 14 |
| Creier | [crates/jarvis-agent](crates/jarvis-agent) | ciclul „observă → acționează → verifică”, confirmări, strălucire | ✅ | 46 |
| Furnizori LLM | [crates/jarvis-llm](crates/jarvis-llm) | 16 furnizori (Groq, OpenCode Zen, OpenRouter, OpenAI, Anthropic, Gemini…), verificarea cheii, alegerea celor mai bune modele, lanț de rezervă | ✅ | 69 (+3 live) |
| Internet | [crates/jarvis-web](crates/jarvis-web) | căutare (DuckDuckGo fără cheie, Brave, Tavily…), citirea paginilor, cercetare cu surse, protecție împotriva „instrucțiunilor” malițioase | ✅ | 64 (+2 live) |
| PC | [crates/jarvis-pc](crates/jarvis-pc) | mouse, tastatură, UI Automation, capturi de ecran cu marcaje, OCR, terminal | ✅ | 83 (+8 intrare live) |
| Bază de cunoștințe | [crates/jarvis-kb](crates/jarvis-kb), [tools/kb-build](tools/kb-build), [kb/seed](kb/seed) | ~20 mii de fragmente din documentația Microsoft + fișe; Recall@5 = 95,4 % | ✅ | 72 |
| Interfață | [src](src), [src-tauri](src-tauri) | HUD, widget-reactor în colț, overlay-strălucire, setări, simulator demo | ✅ (netestată live) | 39 + 25 |
| Voce | [crates/jarvis-voice](crates/jarvis-voice) | client TTS, normalizator de vorbire, VAD, „Jarvis / stop / da”, filtru de ecou, clienți STT | 🟡 fără difuzoare, microfon și lanț complet | 54 (+3 live) |
| Memorie | [crates/jarvis-memory](crates/jarvis-memory) | fapte, lecții din greșeli, playbook-uri, auto-învățare | ⏸ în lucru | — |
| Sistem | [crates/jarvis-sys](crates/jarvis-sys) | niveluri de risc ale comenzilor, PowerShell, anulare, resurse, căutare și deschidere | ⏸ în lucru | — |
| Server STT | [server/jarvis-stt-rs](server/jarvis-stt-rs) | recunoașterea vorbirii în rusă pe Xeon | ⏸ în lucru | — |
| Server TTS | [server/jarvis-tts-rs](server/jarvis-tts-rs) | sinteza vocii `jarvis-ru` (rulează pe Xeon) | ✅ funcționează | — |

Testele „live” accesează rețeaua sau mișcă mouse-ul și sunt sărite implicit (`#[ignore]`).

## Serverul de voce (Xeon 192.168.50.38)

- **TTS**: `https://192.168.50.38:8443`, fără token, TLS cu CA privat ([src-tauri/config/xeon-tts-ca.pem](src-tauri/config/xeon-tts-ca.pem)). `GET /ready`, `POST /tts/stream` → PCM 24 kHz. Modelul se descarcă de pe server în `server/jarvis-tts-rs/model/` (nu se păstrează în git).
- **STT**: va fi la `https://192.168.50.38:8444`; protocolul — [crates/jarvis-core/src/stt_protocol.rs](crates/jarvis-core/src/stt_protocol.rs).

## Configurare

Totul se află într-un singur fișier `config.json` în rădăcina proiectului (în `.gitignore`, nu se comite). Un exemplu fără secrete este [config.example.json](config.example.json):

```powershell
copy config.example.json config.json
```

Acolo sunt furnizorii LLM și cheile, lanțurile de modele pe roluri (`brain`, `fast`, `vision`, `reflection`), căutarea, adresele TTS/STT, permisiunile (`autonomy`: `ask_dangerous` — întreabă doar despre acțiunile periculoase) și modul ferestrelor.

Datele lui Jarvis (memorie, bază de cunoștințe, jurnal de anulare) se află în `data/` (tot în afara git).

## Pornire și teste

Necesar: Rust (stable), Node.js 24, WebView2, `cargo install tauri-cli`.

```powershell
# interfața cu simulatorul demo (backend-ul nu este încă conectat)
npm install
cargo tauri dev

# testele unui modul (fiecare crate se compilează separat)
cd crates\jarvis-agent; cargo test
npm test                     # teste frontend

# baza de cunoștințe Windows
cd tools\kb-build; cargo run --release -- --offline   # reconstruire din ce e deja descărcat
cargo run --release -- --eval                          # măsurarea preciziei
```

Taste rapide: **Ctrl+Alt+J** — HUD pe tot ecranul / în colț, **Ctrl+Alt+K** — oprire de urgență.

## Securitate

- Acțiunile sunt împărțite pe niveluri: **T0** citire, **T1** acțiune (cu strălucire), **T2** modificări în profilul utilizatorului (cu anulare), **T3** periculos — Jarvis spune ce urmează să facă și așteaptă „da, domnule”. O comandă necunoscută este considerată periculoasă.
- Textul de pe site-uri, din terminal și de pe ecran este dată, nu comandă: Jarvis nu execută „instrucțiunile” găsite acolo.
- Cheile și parolele sunt eliminate din text înainte de a fi trimis în cloud.
- Cheile se păstrează doar în `config.json` și sunt mascate în interfață și în jurnale.

## Pașii următori

1. Finalizarea modulelor Memorie, Sistem, Voce și a serverului STT.
2. Integrare: conectarea modulelor reale în locul datelor demo din interfață, un workspace Cargo comun.
3. Teste end-to-end după criteriile de acceptare din [docs/PLAN.md](docs/PLAN.md), prima pornire live.

"use client";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  useConsole,
  type Console,
  type Settings,
  type Phase,
} from "@/lib/console";
import { CoreDisplay } from "./core-display";
import { LiveSpeech } from "./live-speech";
import { Icon } from "./icons";
const states: Record<Phase, string> = {
  IDLE: "Ожидаю команду",
  STOPPED: "Остановлено",
  LISTENING: "Слушаю вас",
  PROCESSING: "Обрабатываю запрос",
  SPEAKING: "Говорю",
  EXECUTING: "Выполняю действие",
  ERROR: "Требуется внимание",
};
const titles: Record<string, string> = {
  center: "Командный центр",
  memory: "Память",
  settings: "Настройки",
};
function Clock() {
  const [now, setNow] = useState("");
  useEffect(() => {
    const tick = () => setNow(new Date().toLocaleTimeString("ru-RU"));
    tick();
    const t = setInterval(tick, 1000);
    return () => clearInterval(t);
  }, []);
  return <time>{now || "—"}</time>;
}
function PanelTitle({
  icon,
  children,
}: {
  icon: string;
  children: React.ReactNode;
}) {
  return (
    <h3>
      <Icon name={icon} size={14} />
      {children}
    </h3>
  );
}
function Flag({ v }: { v: boolean | null }) {
  return (
    <i className={"light " + (v === null ? "unknown" : v ? "ok" : "off")} />
  );
}
function Rail({ c }: { c: Console }) {
  const t = c.stats;
  return (
    <aside className="rail">
      <section className="panel">
        <PanelTitle icon="connection">Подсистемы</PanelTitle>
        {[
          [
            "JARVIS AI v2 — Цезарь",
            "Интеллектуальное ядро",
            c.links ? !c.links.llmError : null,
          ],
          [
            "Голосовой движок",
            "Rust TTS · 24 kHz",
            c.links ? !!c.links.tts?.ready : null,
          ],
          ["Распознавание", c.streamingStt && c.speech.state !== "fallback" ? "T-one · RU" : "Zipformer · RU", c.online ? c.stt : null],
          [
            "Микрофон",
            c.mic === "off" ? "Выключен" : "Активен",
            c.mic === "off" ? null : true,
          ],
        ].map(([k, v, on]) => (
          <div className="subsystem" key={String(k)}>
            <div>
              <b>{k}</b>
              <small>{v}</small>
            </div>
            <Flag v={on as boolean | null} />
          </div>
        ))}
      </section>
      <section className="panel">
        <PanelTitle icon="activity">Телеметрия Mac</PanelTitle>
        {[
          ["CPU", t?.cpu],
          ["RAM", t ? (100 * t.ram) / t.total : undefined],
        ].map(([k, val]) => (
          <div className="metric" key={k}>
            <label>
              {k}
              <b>{typeof val === "number" ? val.toFixed(0) + "%" : "—"}</b>
            </label>
            <div className="meter">
              <i style={{ width: (Number(val) || 0) + "%" }} />
            </div>
          </div>
        ))}
        <small className="mono dim">
          {t
            ? (t.ram / 1073741824).toFixed(1) +
              " / " +
              (t.total / 1073741824).toFixed(1) +
              " GiB"
            : "— GiB"}
        </small>
        <div className="gpu">
          <small>GPU</small>
          <b>{t?.gpu || "—"}</b>
        </div>
      </section>
    </aside>
  );
}
function Transcript({ c }: { c: Console }) {
  const history = useRef<HTMLDivElement>(null);
  const followLatest = useRef(true);
  useLayoutEffect(() => {
    followLatest.current = true;
  }, [c.session]);
  useLayoutEffect(() => {
    const box = history.current;
    if (box && followLatest.current) box.scrollTop = box.scrollHeight;
  }, [c.messages, c.partial]);
  return (
    <section className="panel timeline">
      <div className="timelineHead">
        <PanelTitle icon="terminal">Диалог</PanelTitle>
        <button
          className="iconButton"
          title="Новый диалог"
          aria-label="Новый диалог"
          onClick={() => void c.manage()}
        >
          <Icon name="plus" />
        </button>
      </div>
      <select
        className="sessionSelect"
        aria-label="Активный диалог"
        value={c.session}
        onChange={(e) => void c.select(e.target.value).catch(c.report)}
      >
        {c.sessions.map((s) => (
          <option value={s.id} key={s.id}>
            {s.title}
          </option>
        ))}
      </select>
      <div
        ref={history}
        className="messages"
        aria-label="История диалога"
        onScroll={(e) => {
          const box = e.currentTarget;
          followLatest.current = box.scrollHeight - box.clientHeight - box.scrollTop <= 16;
        }}
      >
        {!c.messages.length && !c.partial ? (
          <div className="empty">
            <Icon name="chat" size={28} />
            <b>Диалог открыт</b>
            <span>Произнесите или напишите первую команду.</span>
          </div>
        ) : null}
        {c.messages.map((m, i) => (
          <article key={i} className={m.role}>
            <small>{m.role === "user" ? "ВЫ" : "J.A.R.V.I.S."}</small>
            <p>{m.content}</p>
          </article>
        ))}
        {c.partial && (
          <article className="assistant">
            <small>J.A.R.V.I.S.</small>
            <p>
              {c.partial}
              <i className="cursor" />
            </p>
          </article>
        )}
      </div>
    </section>
  );
}
function SettingsForm({ c }: { c: Console }) {
  const [s, set] = useState<Settings>(c.settings),
    [saved, notice] = useState("");
  function field<K extends keyof Settings>(k: K, value: Settings[K]) {
    set((p) => ({ ...p, [k]: value }));
    notice("");
  }
  const known: Record<string, [string, string]> = {
    Groq: ["https://api.groq.com/openai/v1", "openai/gpt-oss-120b"],
    OmniRouter: [
      "https://venues-gain-omissions-often.trycloudflare.com/v1",
      "opencode-zen/deepseek-v4.1-flash",
    ],
    "OpenCode Zen": ["https://opencode.ai/zen/v1", ""],
    "Z.AI": ["https://api.z.ai/api/paas/v4", ""],
    Совместимый: ["", ""],
  };
  return (
    <form
      className="panel formPanel"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await c.save(s);
          notice("Настройки сохранены");
        } catch (err) {
          c.report(err);
        }
      }}
    >
      <PanelTitle icon="gear">Модель и личность</PanelTitle>
      <div className="fields">
        <label>
          Провайдер
          <select
            value={s.provider}
            onChange={(e) => {
              const n = e.target.value;
              set({ ...s, provider: n, url: known[n][0], model: known[n][1] });
              notice("");
            }}
          >
            {Object.keys(known).map((k) => (
              <option key={k}>{k}</option>
            ))}
          </select>
        </label>
        <label>
          Модель
          <input
            list="models"
            value={s.model}
            required
            onChange={(e) => field("model", e.target.value)}
          />
          <datalist id="models">
            {c.links?.models.map((m) => (
              <option key={m}>{m}</option>
            ))}
          </datalist>
        </label>
      </div>
      <label>
        Базовый URL API
        <input
          value={s.url}
          required
          type="url"
          onChange={(e) => field("url", e.target.value)}
        />
      </label>
      <label>
        Системный промпт
        <textarea
          rows={4}
          value={s.prompt}
          onChange={(e) => field("prompt", e.target.value)}
        />
      </label>
      <label className="toggle">
        <input
          type="checkbox"
          checked={s.speak}
          onChange={(e) => field("speak", e.target.checked)}
        />
        Озвучивать ответы голосом Джарвиса
      </label>
      <label className="toggle">
        <input
          type="checkbox"
          checked={s.tools}
          onChange={(e) => field("tools", e.target.checked)}
        />
        {c.windows ? "Разрешить модели инструменты Windows" : "Разрешить модели диагностические инструменты"}
      </label>
      {c.windows && <label className="auto"><input type="checkbox" checked={s.read_only} onChange={e=>field("read_only",e.target.checked)} />Только чтение</label>}
      <label>
        Пауза после речи
        <select value={s.silence_ms} onChange={(e) => field("silence_ms", Number(e.target.value))}>
          {[800, 900, 1000, 1100, 1200].map((ms) => <option key={ms} value={ms}>{ms} мс</option>)}
        </select>
      </label>
      <label className="toggle">
        <input type="checkbox" checked={s.wake_word}
          onChange={(e) => field("wake_word", e.target.checked)} />
        При запуске включать микрофон и ждать обращения «Джарвис»
      </label>
      <div className="actions">
        <button className="primary">Сохранить настройки</button>
        <span role="status">{saved}</span>
      </div>
      <p className="hint">
        Ключи и доверенные адреса провайдеров задаются в backend.env приложения.
      </p>
    </form>
  );
}
function Memory({ c }: { c: Console }) {
  const [text, set] = useState(""),
    [del, confirm] = useState("");
  return (
    <div className="memoryPage">
      <section className="panel formPanel">
        <PanelTitle icon="memory">Сохранённые предпочтения</PanelTitle>
        <p className="hint">Эти сведения входят в контекст новых ответов.</p>
        {c.prefs.map((p) => (
          <div className="listRow" key={p.id}>
            {p.body}
            <button
              aria-label="Забыть предпочтение"
              onClick={() => void c.preference(undefined, p.id).catch(c.report)}
            >
              <Icon name="trash" size={15} />
            </button>
          </div>
        ))}
        <form
          className="composer"
          onSubmit={(e) => {
            e.preventDefault();
            void c
              .preference(text)
              .then(() => set(""))
              .catch(c.report);
          }}
        >
          <input
            placeholder="Что Джарвису запомнить?"
            value={text}
            onChange={(e) => set(e.target.value)}
            maxLength={250}
          />
          <button disabled={!text.trim()} aria-label="Сохранить предпочтение">
            <Icon name="plus" />
          </button>
        </form>
      </section>
      <section className="panel formPanel">
        <PanelTitle icon="chat">Сессии диалога</PanelTitle>
        {c.sessions.map((s) => (
          <div className="listRow" key={s.id}>
            <span>{s.title}</span>
            <button
              onClick={() =>
                del === s.id
                  ? void c.manage(s.id).then(() => confirm(""))
                  : confirm(s.id)
              }
            >
              {del === s.id ? "Подтвердить удаление" : "Удалить"}
            </button>
          </div>
        ))}
      </section>
    </div>
  );
}
function Diagnostics({ c }: { c: Console }) {
  const [name, set] = useState("get_time"),
    [result, answer] = useState(""),
    [busy, working] = useState(false);
  const options = {
    get_time: "Местное время",
    system_info: "Информация о системе",
    cpu: "Процессор",
    ram: "Память",
    disks: "Диски",
    server_status: "Соединение с сервером",
    tts_status: "Готовность TTS",
  };
  return (
    <section className="panel formPanel">
      <PanelTitle icon="terminal">Диагностика</PanelTitle>
      <div className="toolLine">
        <select value={name} onChange={(e) => set(e.target.value)}>
          {Object.entries(options).map(([k, v]) => (
            <option key={k} value={k}>
              {v}
            </option>
          ))}
        </select>
        <button
          disabled={busy}
          onClick={async () => {
            working(true);
            try {
              answer(
                JSON.stringify(await c.act("run_tool", { name }), null, 2),
              );
            } catch (e) {
              answer(String(e));
            } finally {
              working(false);
            }
          }}
        >
          Выполнить
        </button>
      </div>
      {result && <pre>{result}</pre>}
    </section>
  );
}
export default function Hud() {
  const c = useConsole();
  const [page, tab] = useState("center"),
    [text, set] = useState("");
  const voice = useRef(c.capture);
  useEffect(() => {
    voice.current = c.capture;
  }, [c.capture]);
  useEffect(() => {
    let held = false;
    const editable = (t: EventTarget | null) =>
      t instanceof HTMLElement &&
      (t.isContentEditable ||
        ["INPUT", "TEXTAREA", "SELECT", "BUTTON"].includes(t.tagName));
    const down = (e: KeyboardEvent) => {
      if (
        e.code === "Space" &&
        !e.repeat &&
        !editable(e.target) &&
        page === "center"
      ) {
        held = true;
        e.preventDefault();
        void voice.current("ptt");
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.code === "Space" && held) {
        held = false;
        e.preventDefault();
        void voice.current("submit");
      }
    };
    const blur = () => {
      if (held) {
        held = false;
        void voice.current("off");
      }
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", blur);
      blur();
    };
  }, [page]);
  return (
    <div className={page === "center" ? "shell commandCenter" : "shell"}>
      {c.proposal && <div className="actionShade"><section role="alertdialog" aria-modal="true" aria-label="Подтверждение действия" className="panel actionPrompt">
        <h2>Подтвердить действие?</h2>
        <p>{{fs_write:"Записать файл",fs_move:"Переименовать файл",fs_copy:"Скопировать файл",undo_last:"Отменить последнее изменение"}[c.proposal.tool] ?? c.proposal.tool}</p>
        <p>{c.proposal.detail.folder}</p><strong>{c.proposal.detail.file}</strong>{c.proposal.detail.destination && <> → {c.proposal.detail.destination}</>}
        {c.proposal.detail.text_after!==null && <pre>{c.proposal.detail.text_after}</pre>}
        <div><button autoFocus onClick={()=>void c.decide(false)}>Отмена</button> <button onClick={()=>void c.decide(true)}>Подтвердить</button></div>
      </section></div>}
      <header>
        <div className="brand">
          <span className="brandMark">
            <Icon name="core" size={26} />
          </span>
          J.A.R.V.I.S.
        </div>
        <nav>
          {Object.entries(titles).map(([id, t]) => (
            <button
              key={id}
              aria-current={page === id ? "page" : undefined}
              onClick={() => tab(id)}
            >
              <Icon
                name={
                  id === "center"
                    ? "activity"
                    : id === "memory"
                      ? "memory"
                      : "gear"
                }
                size={15}
              />
              {t}
            </button>
          ))}
        </nav>
        <div className="clock">
          <Flag v={c.online || null} />
          <Clock />
        </div>
      </header>
      <main aria-label={titles[page]}>
        {page !== "center" && (
          <div className="pageTitle">
            <div>
              <small className="kicker">
                <i /> JARVIS OS <span>/</span>{" "}
                {page === "memory" ? "MEMORY" : "CONFIGURATION"}
              </small>
              <h1>{titles[page]}</h1>
            </div>
            <div className="host">
              <Icon name="shield" size={15} />
              {c.stats?.host || "Локальный клиент"}
            </div>
          </div>
        )}
        {c.error && (
          <div role="alert" className="alert">
            <span>{c.error}</span>
            <button
              aria-label="Скрыть сообщение"
              onClick={() => c.setError("")}
            >
              ×
            </button>
          </div>
        )}
        {page === "center" ? (
          <>
            <div className="workspace">
              <Rail c={c} />
              <section className="stage">
                <div className="stageTop">
                  <span>
                    <i /> ГОЛОСОВОЕ ЯДРО
                  </span>
                  <span>PCM / 24K</span>
                </div>
                <div className="orbBox">
                  <CoreDisplay phase={c.phase} energy={c.energy} />
                </div>
                <div className="coreStatus">
                  <span className="phase">{c.phase}</span>
                  <h2>{states[c.phase]}</h2>
                </div>
                <LiveSpeech
                  speech={c.speech}
                  levels={c.micLevels}
                  active={c.mic !== "off"}
                  recognizing={c.phase === "PROCESSING" && !c.busy}
                />
                <div className="voiceControl">
                  <button
                    className="primary"
                    disabled={!c.online}
                    title={c.mic === "off" ? "Начать голосовую команду" : "Закончить запись и отправить распознанную команду"}
                    onClick={() =>
                      void c.capture(c.mic === "off" ? "ptt" : "submit")
                    }
                  >
                    <Icon name={c.mic === "off" ? "mic" : "stop"} />
                    {c.mic === "off" ? "Говорить" : "Завершить"}
                  </button>
                  <button
                    className="stopControl"
                    title="Отменить запись и остановить генерацию и озвучивание"
                    disabled={!c.online || (c.mic === "off" && !c.busy && c.phase !== "PROCESSING")}
                    onClick={() => void c.stop()}
                  >
                    <Icon name="stop" />
                    Остановить
                  </button>
                </div>
                <label className="auto">
                  <input
                    type="checkbox"
                    checked={c.mic === "auto"}
                    disabled={!c.online}
                    onChange={(e) =>
                      void c.capture(e.target.checked ? "auto" : "off")
                    }
                  />
                  {c.settings.wake_word ? "По обращению «Джарвис»" : "Постоянное прослушивание"}
                </label>
              </section>
              <Transcript c={c} />
            </div>
            <form
              className="command"
              onSubmit={(e) => {
                e.preventDefault();
                const msg = text.trim();
                if (msg) {
                  set("");
                  void c.send(msg);
                }
              }}
            >
              <span className="commandIcon">
                <Icon name="terminal" />
              </span>
              <input
                aria-label="Текст команды"
                placeholder="Введите команду Джарвису…"
                value={text}
                onChange={(e) => set(e.target.value)}
                maxLength={4000}
              />
              <button
                className="submit"
                disabled={!text.trim() || !c.online}
                aria-label="Отправить команду"
              >
                <Icon name="send" size={18} />
              </button>
            </form>
          </>
        ) : page === "settings" ? (
          <div className="settingsPage">
            <SettingsForm key={c.settings.provider + c.online} c={c} />
            <Diagnostics c={c} />
          </div>
        ) : (
          <Memory c={c} />
        )}
      </main>
      <footer>
        <span>
          <i className="light ok" /> {c.online ? "TAURI CORE" : "WEB VIEW"}
        </span>
        <span>
          {c.firstMs == null
            ? "Голосовой выход · PCM16"
            : "Первый звук ответа · " + c.firstMs + " мс"}
        </span>
        <span>JARVIS OS / 01</span>
      </footer>
    </div>
  );
}

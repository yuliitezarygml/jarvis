"use client";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

export type Settings = {
  provider: string;
  url: string;
  model: string;
  prompt: string;
  speak: boolean;
  tools: boolean;
};
export type Msg = { role: "user" | "assistant"; content: string };
export type Session = { id: string; title: string };
type Pref = { id: number; body: string };
export type Phase =
  | "IDLE"
  | "LISTENING"
  | "THINKING"
  | "SPEAKING"
  | "EXECUTING"
  | "ERROR";
type Bootstrap = {
  sessions: Session[];
  settings: Settings;
  preferences: Pref[];
  stt: boolean;
  microphone: number;
};
type Link = {
  models: string[];
  llmError: string | null;
  tts: { ready: boolean; sample_rate: number } | null;
  ttsError: string | null;
};
type Stats = {
  cpu: number;
  ram: number;
  total: number;
  gpu: string;
  host: string;
  os: string;
  cores: number;
};
type Event = { session: string; turn?: string; kind: string; data?: unknown };
export const defaults: Settings = {
  provider: "Groq",
  url: "",
  model: "",
  prompt: "",
  speak: true,
  tools: false,
};
export function useConsole() {
  const [online, setOnline] = useState(false),
    [sessions, setSessions] = useState<Session[]>([]),
    [session, setSession] = useState(""),
    [messages, setMessages] = useState<Msg[]>([]);
  const [settings, setSettings] = useState<Settings>(defaults),
    [prefs, setPrefs] = useState<Pref[]>([]),
    [stats, setStats] = useState<Stats | null>(null),
    [links, setLinks] = useState<Link | null>(null);
  const [phase, setPhase] = useState<Phase>("IDLE"),
    [mic, setMic] = useState("off"),
    [stt, setStt] = useState(false),
    [permission, setPermission] = useState(0),
    [error, setError] = useState(""),
    [firstMs, setFirstMs] = useState<number | null>(null);
  const [partial, setPartial] = useState(""),
    [busy, setBusy] = useState(false);
  const energy = useRef(0),
    sid = useRef(""),
    turn = useRef(""),
    liveMic = useRef("off"),
    micChain = useRef<Promise<void>>(Promise.resolve()),
    buffer = useRef(""),
    paint = useRef(0);
  const report = useCallback((e: unknown) => setError(String(e)), []);
  const updateHistory = useCallback(async (id: string) => {
    const rows = await invoke<Msg[]>("messages", { session: id });
    if (sid.current === id) setMessages(rows);
  }, []);
  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setLinks(await invoke<Link>("connections"));
    } catch (e) {
      report(e);
    }
  }, [report]);
  const load = useCallback(async () => {
    const data = await invoke<Bootstrap>("bootstrap");
    setSessions(data.sessions);
    setSettings(data.settings);
    setPrefs(data.preferences);
    setStt(data.stt);
    setPermission(data.microphone);
    if (!sid.current && data.sessions.length) {
      sid.current = data.sessions[0].id;
      setSession(sid.current);
      await updateHistory(sid.current);
    }
  }, [updateHistory]);
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    let remove: (() => void) | undefined;
    const register = async () => {
      const off = await listen<Event>("jarvis-event", ({ payload: e }) => {
        if (e.session !== sid.current) return;
        if (e.kind === "start") {
          turn.current = e.turn ?? "";
          buffer.current = "";
          setPartial("");
          setError("");
          setFirstMs(null);
          setPhase("THINKING");
          setBusy(true);
          setMessages((m) => [...m, { role: "user", content: String(e.data) }]);
          return;
        }
        if (e.turn && e.turn !== turn.current) return;
        switch (e.kind) {
          case "delta":
            buffer.current += String(e.data);
            if (!paint.current)
              paint.current = requestAnimationFrame(() => {
                paint.current = 0;
                setPartial(buffer.current);
              });
            break;
          case "speaking":
            setPhase("SPEAKING");
            setFirstMs((e.data as { ms: number }).ms);
            break;
          case "thinking":
            setPhase("THINKING");
            break;
          case "executing":
            setPhase("EXECUTING");
            break;
          case "level":
          case "mic_level":
            energy.current = Math.min(1, Number(e.data) * 5);
            break;
          case "mic":
            liveMic.current = String(e.data);
            setMic(liveMic.current);
            setPermission(3);
            if (liveMic.current !== "off") setPhase("LISTENING");
            else if (!turn.current) setPhase("IDLE");
            break;
          case "voice_start":
            setPhase("LISTENING");
            break;
          case "recognizing":
            setPhase("THINKING");
            break;
          case "mic_error":
            setError(String(e.data));
            setPhase("ERROR");
            break;
          case "warning":
            setError(String(e.data));
            break;
          case "done":
          case "cancelled":
          case "error":
            turn.current = "";
            setBusy(false);
            buffer.current = "";
            setPartial("");
            energy.current = 0;
            setPhase(
              e.kind === "error"
                ? "ERROR"
                : liveMic.current === "off"
                  ? "IDLE"
                  : "LISTENING",
            );
            if (e.kind === "error") setError(String(e.data));
            void updateHistory(e.session).catch(report);
            void load().catch(report);
            break;
        }
      });
      if (!alive) {
        off();
        return;
      }
      remove = off;
      setOnline(true);
      await load();
      await refresh();
    };
    void register().catch(report);
    const poll = () => {
      if (!document.hidden)
        void invoke<Stats>("telemetry")
          .then((v) => {
            if (alive) setStats(v);
          })
          .catch(report);
    };
    poll();
    const timer = setInterval(poll, 2500);
    const network = setInterval(() => {
      if (!document.hidden) void refresh();
    }, 45000);
    return () => {
      alive = false;
      remove?.();
      clearInterval(timer);
      clearInterval(network);
      cancelAnimationFrame(paint.current);
    };
  }, [load, refresh, report, updateHistory]);
  async function act<T>(cmd: string, args?: Record<string, unknown>) {
    if (!isTauri()) throw Error("Откройте приложение Jarvis на Mac");
    return invoke<T>(cmd, args);
  }
  async function stop() {
    try {
      await act("interrupt");
      setBusy(false);
      setPhase("IDLE");
      energy.current = 0;
    } catch (e) {
      report(e);
    }
  }
  async function select(id: string) {
    if (!id) return;
    await stop();
    sid.current = id;
    turn.current = "";
    setSession(id);
    setPartial("");
    setMessages([]);
    setError("");
    await updateHistory(id);
  }
  async function manage(remove?: string) {
    try {
      const v = await act<{ sessions: Session[]; id: string | null }>(
        "session_action",
        { remove: remove ?? null },
      );
      setSessions(v.sessions);
      await select(v.id ?? v.sessions[0].id);
    } catch (e) {
      report(e);
    }
  }
  async function send(text: string) {
    if (!text.trim()) return;
    try {
      await act("send_text", { session: sid.current, text });
    } catch (e) {
      report(e);
      setPhase("ERROR");
    }
  }
  async function capture(mode: string) {
    const id = sid.current;
    micChain.current = micChain.current.then(async () => {
      if (sid.current !== id) return;
      try {
        setError("");
        await act("microphone", { session: id, mode });
      } catch (e) {
        report(e);
        setPhase("ERROR");
      }
    });
    await micChain.current;
  }
  async function save(s: Settings) {
    await act("save_settings", { settings: s });
    setSettings(s);
    void refresh();
  }
  async function preference(text?: string, forget?: number) {
    const p = await act<Pref[]>("preferences", {
      text: text ?? null,
      forget: forget ?? null,
    });
    setPrefs(p);
  }
  return {
    online,
    sessions,
    session,
    messages,
    settings,
    prefs,
    stats,
    links,
    phase,
    mic,
    stt,
    permission,
    error,
    firstMs,
    partial,
    busy,
    energy,
    refresh,
    select,
    manage,
    send,
    stop,
    capture,
    save,
    preference,
    report,
    setError,
    act,
  };
}
export type Console = ReturnType<typeof useConsole>;

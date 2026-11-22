// Состояние интерфейса, собранное из потока событий `jarvis://event`. Чистый редьюсер: (state, event, now) → state.
// Неизвестные и битые события игнорируются (возвращается тот же объект состояния).

import type {
  ActionStatus,
  AssistantState,
  AudioSource,
  JarvisEvent,
  Rect,
  RiskTier,
  SourceLink,
  Speaker,
} from './types';

export const MAX_ACTIONS = 100;
export const MAX_SUBTITLE_HISTORY = 40;
export const MAX_ERRORS = 20;
export const TELEMETRY_POINTS = 60;

export interface SubtitleLine {
  speaker: Speaker;
  text: string;
  final: boolean;
  at: number;
}

export interface ActionItem {
  id: string;
  tool: string;
  summary: string;
  status: ActionStatus;
  startedAt: number;
  updatedAt: number;
}

export interface ControlState {
  active: boolean;
  tier: RiskTier | null;
  since: number;
  target: { rect: Rect; label: string; at: number } | null;
}

export interface ConfirmState {
  id: string;
  summary: string;
  tier: RiskTier;
  timeoutMs: number;
  receivedAt: number;
}

export interface TelemetrySample {
  cpu: number;
  ramUsedMb: number;
  ramTotalMb: number;
  gpu: number | null;
  netKbps: number | null;
  at: number;
}

export interface TelemetryHistory {
  cpu: number[];
  ram: number[];
  gpu: number[];
  net: number[];
}

export interface AudioState {
  mic: number;
  tts: number;
  lastSource: AudioSource | null;
  at: number;
}

export interface JarvisState {
  assistant: AssistantState;
  stateSince: number;
  subtitle: SubtitleLine | null;
  subtitleHistory: SubtitleLine[];
  audio: AudioState;
  control: ControlState;
  actions: ActionItem[];
  confirm: ConfirmState | null;
  lastConfirm: { id: string; approved: boolean; at: number } | null;
  sources: SourceLink[];
  telemetry: TelemetrySample | null;
  telemetryHistory: TelemetryHistory;
  errors: { message: string; at: number }[];
  lastError: { message: string; at: number } | null;
}

export const initialState: JarvisState = {
  assistant: 'idle',
  stateSince: 0,
  subtitle: null,
  subtitleHistory: [],
  audio: { mic: 0, tts: 0, lastSource: null, at: 0 },
  control: { active: false, tier: null, since: 0, target: null },
  actions: [],
  confirm: null,
  lastConfirm: null,
  sources: [],
  telemetry: null,
  telemetryHistory: { cpu: [], ram: [], gpu: [], net: [] },
  errors: [],
  lastError: null,
};

const STATES: readonly AssistantState[] = ['idle', 'listening', 'thinking', 'speaking', 'controlling', 'error'];
const TIERS: readonly RiskTier[] = ['T0', 'T1', 'T2', 'T3'];
const STATUSES: readonly ActionStatus[] = ['running', 'done', 'failed', 'cancelled', 'awaiting_confirm'];

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null;
const isStr = (v: unknown): v is string => typeof v === 'string';
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

function isRect(v: unknown): v is Rect {
  return isObj(v) && isNum(v.x) && isNum(v.y) && isNum(v.w) && isNum(v.h);
}

/** Проверка формы события (из Rust приходит корректное, но фронтенд не должен падать на чужом JSON). */
export function parseEvent(raw: unknown): JarvisEvent | null {
  if (!isObj(raw) || !isStr(raw.type)) return null;
  const e = raw;
  switch (e.type) {
    case 'state':
      return STATES.includes(e.state as AssistantState) ? (e as JarvisEvent) : null;
    case 'subtitle':
      return (e.speaker === 'user' || e.speaker === 'jarvis') && isStr(e.text) && typeof e.is_final === 'boolean'
        ? (e as JarvisEvent)
        : null;
    case 'audio_level':
      return (e.source === 'mic' || e.source === 'tts') && isNum(e.rms) ? (e as JarvisEvent) : null;
    case 'control_start':
      return TIERS.includes(e.tier as RiskTier) ? (e as JarvisEvent) : null;
    case 'control_target':
      return isRect(e.rect) && isStr(e.label) ? (e as JarvisEvent) : null;
    case 'control_stop':
      return e as JarvisEvent;
    case 'action':
      return isStr(e.id) && isStr(e.tool) && isStr(e.summary) && STATUSES.includes(e.status as ActionStatus)
        ? (e as JarvisEvent)
        : null;
    case 'confirm_request':
      return isStr(e.id) && isStr(e.summary) && TIERS.includes(e.tier as RiskTier) && isNum(e.timeout_ms)
        ? (e as JarvisEvent)
        : null;
    case 'confirm_resolved':
      return isStr(e.id) && typeof e.approved === 'boolean' ? (e as JarvisEvent) : null;
    case 'telemetry':
      return isNum(e.cpu) && isNum(e.ram_used_mb) && isNum(e.ram_total_mb) && (e.gpu === null || e.gpu === undefined || isNum(e.gpu))
        ? (e as JarvisEvent)
        : null;
    case 'sources':
      return Array.isArray(e.items) && e.items.every((i) => isObj(i) && isStr(i.title) && isStr(i.url))
        ? (e as JarvisEvent)
        : null;
    case 'error':
      return isStr(e.message) ? (e as JarvisEvent) : null;
    default:
      return null;
  }
}

function push<T>(list: T[], item: T, max: number): T[] {
  const next = list.length >= max ? list.slice(list.length - max + 1) : list.slice();
  next.push(item);
  return next;
}

export function reduce(state: JarvisState, raw: unknown, now: number = Date.now()): JarvisState {
  const e = parseEvent(raw);
  if (!e) return state;
  switch (e.type) {
    case 'state':
      if (e.state === state.assistant) return state;
      return { ...state, assistant: e.state, stateSince: now };

    case 'subtitle': {
      const line: SubtitleLine = { speaker: e.speaker, text: e.text, final: e.is_final, at: now };
      return {
        ...state,
        subtitle: line,
        subtitleHistory: e.is_final ? push(state.subtitleHistory, line, MAX_SUBTITLE_HISTORY) : state.subtitleHistory,
      };
    }

    case 'audio_level': {
      const rms = clamp01(e.rms);
      return { ...state, audio: { ...state.audio, [e.source]: rms, lastSource: e.source, at: now } };
    }

    case 'control_start':
      return { ...state, control: { active: true, tier: e.tier, since: now, target: null } };

    case 'control_target':
      return {
        ...state,
        control: {
          ...state.control,
          active: true,
          since: state.control.active ? state.control.since : now,
          target: { rect: e.rect, label: e.label, at: now },
        },
      };

    case 'control_stop':
      if (!state.control.active && !state.control.target) return state;
      return { ...state, control: { active: false, tier: null, since: now, target: null } };

    case 'action': {
      const idx = state.actions.findIndex((a) => a.id === e.id);
      if (idx >= 0) {
        const actions = state.actions.slice();
        actions[idx] = { ...actions[idx], tool: e.tool, summary: e.summary, status: e.status, updatedAt: now };
        return { ...state, actions };
      }
      const item: ActionItem = { id: e.id, tool: e.tool, summary: e.summary, status: e.status, startedAt: now, updatedAt: now };
      return { ...state, actions: push(state.actions, item, MAX_ACTIONS) };
    }

    case 'confirm_request':
      return {
        ...state,
        confirm: { id: e.id, summary: e.summary, tier: e.tier, timeoutMs: e.timeout_ms, receivedAt: now },
      };

    case 'confirm_resolved':
      if (!state.confirm || state.confirm.id !== e.id) {
        return { ...state, lastConfirm: { id: e.id, approved: e.approved, at: now } };
      }
      return { ...state, confirm: null, lastConfirm: { id: e.id, approved: e.approved, at: now } };

    case 'telemetry': {
      const ramPct = e.ram_total_mb > 0 ? (e.ram_used_mb / e.ram_total_mb) * 100 : 0;
      const gpu = isNum(e.gpu) ? e.gpu : null;
      const net = isNum(e.net_kbps) ? e.net_kbps : null;
      const h = state.telemetryHistory;
      return {
        ...state,
        telemetry: { cpu: e.cpu, ramUsedMb: e.ram_used_mb, ramTotalMb: e.ram_total_mb, gpu, netKbps: net, at: now },
        telemetryHistory: {
          cpu: push(h.cpu, e.cpu, TELEMETRY_POINTS),
          ram: push(h.ram, ramPct, TELEMETRY_POINTS),
          gpu: gpu === null ? h.gpu : push(h.gpu, gpu, TELEMETRY_POINTS),
          net: net === null ? h.net : push(h.net, net, TELEMETRY_POINTS),
        },
      };
    }

    case 'sources':
      return { ...state, sources: e.items.map((i) => ({ title: i.title, url: i.url })) };

    case 'error': {
      const err = { message: e.message, at: now };
      return { ...state, lastError: err, errors: push(state.errors, err, MAX_ERRORS) };
    }
  }
}

/** Прогнать список событий (снимок `events_replay`). */
export function reduceAll(state: JarvisState, events: unknown[], now: number = Date.now()): JarvisState {
  return events.reduce<JarvisState>((s, e) => reduce(s, e, now), state);
}

/** Действия для ленты: новые сверху. */
export function actionsNewestFirst(state: JarvisState): ActionItem[] {
  return state.actions.slice().reverse();
}

/** Остаток времени на подтверждение, мс (не меньше 0). */
export function confirmRemainingMs(confirm: ConfirmState, now: number): number {
  return Math.max(0, confirm.receivedAt + confirm.timeoutMs - now);
}

// Зеркало контрактов `crates/jarvis-core` (event.rs, config.rs, risk.rs) для фронтенда.
// Rust сериализует `Event` как JSON с полем `type` в snake_case — это и есть payload Tauri-события `jarvis://event`.

export type AssistantState = 'idle' | 'listening' | 'thinking' | 'speaking' | 'controlling' | 'error';
export type Speaker = 'user' | 'jarvis';
export type AudioSource = 'mic' | 'tts';
export type ActionStatus = 'running' | 'done' | 'failed' | 'cancelled' | 'awaiting_confirm';
/** `RiskTier` сериализуется как есть: "T0".."T3". */
export type RiskTier = 'T0' | 'T1' | 'T2' | 'T3';

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface SourceLink {
  title: string;
  url: string;
}

export type JarvisEvent =
  | { type: 'state'; state: AssistantState }
  | { type: 'subtitle'; speaker: Speaker; text: string; is_final: boolean }
  | { type: 'audio_level'; source: AudioSource; rms: number }
  | { type: 'control_start'; tier: RiskTier }
  | { type: 'control_target'; rect: Rect; label: string }
  | { type: 'control_stop' }
  | { type: 'action'; id: string; tool: string; summary: string; status: ActionStatus }
  | { type: 'confirm_request'; id: string; summary: string; tier: RiskTier; timeout_ms: number }
  | { type: 'confirm_resolved'; id: string; approved: boolean }
  | {
      type: 'telemetry';
      cpu: number;
      ram_used_mb: number;
      ram_total_mb: number;
      gpu: number | null;
      /** Нет в jarvis-core (пока): принимается, если бэкенд начнёт присылать. */
      net_kbps?: number | null;
    }
  | { type: 'sources'; items: SourceLink[] }
  | { type: 'error'; message: string };

export type JarvisEventType = JarvisEvent['type'];

export const EVENT_NAME = 'jarvis://event';
export const CURSOR_EVENT = 'jarvis://cursor';
export const HUD_MODE_EVENT = 'jarvis://hud_mode';
export const EMERGENCY_EVENT = 'jarvis://emergency_stop';
export const SETTINGS_TAB_EVENT = 'jarvis://settings_tab';
/** config.json изменили снаружи (payload — замаскированный `Config`). */
export const CONFIG_CHANGED_EVENT = 'jarvis://config_changed';
/** Уровень микрофона во время «Проверить микрофон» (RMS 0…1). */
export const MIC_TEST_EVENT = 'jarvis://mic_test_level';

// ───────────── config.json (jarvis_core::Config) ─────────────

export type Autonomy = 'ask_dangerous' | 'ask_all_changes' | 'never';
export type ProviderKind = 'openai_compat' | 'anthropic';
export type HudMode = 'fullscreen' | 'corner';

export interface ProviderConfig {
  id: string;
  kind: ProviderKind;
  base_url: string;
  /** В UI всегда маска (`gsk…abcd`) или новый ключ, который пользователь только что вставил. */
  api_key: string;
  enabled: boolean;
}

export interface Roles {
  brain: string[];
  fast: string[];
  vision: string[];
  reflection: string[];
}
export type RoleName = keyof Roles;

export interface SearchProviderConfig {
  id: string;
  api_key: string;
  extra: string;
  enabled: boolean;
}

export interface Config {
  data_dir: string;
  tts: { url: string; voice: string; ca_path: string };
  stt: { engine: string; url: string; ca_path: string };
  providers: ProviderConfig[];
  roles: Roles;
  web_search: SearchProviderConfig[];
  autonomy: Autonomy;
  run_as_admin: boolean;
  vision_enabled: boolean;
  terminal_autorun: boolean;
  resource_guard: boolean;
  search_roots: string[];
  hud: { mode: HudMode; corner_x: number | null; corner_y: number | null; wallpaper: boolean };
  audio: AudioConfig;
  appearance: AppearanceConfig;
}

export type CornerSize = 'small' | 'medium' | 'large';

/** Внешний вид (jarvis_core::config::AppearanceConfig). */
export interface AppearanceConfig {
  /** id темы из `THEMES` (src/lib/theme.ts). */
  theme: string;
  /** Свой акцент `#RRGGBB` поверх темы. */
  accent: string | null;
  /** Яркость свечения 0.3…1.5. */
  glow: number;
  /** Скорость анимации реактора 0.25…2. */
  animation_speed: number;
  corner_size: CornerSize;
  /** Сканлинии и гекс-сетка на фоне HUD. */
  scanlines: boolean;
  show_telemetry: boolean;
  show_actions: boolean;
  show_sources: boolean;
  show_subtitles: boolean;
}

/** Микрофон, динамики, громкость (jarvis_core::config::AudioConfig). `null` — устройство Windows по умолчанию. */
export interface AudioConfig {
  input_device: string | null;
  output_device: string | null;
  /** 0…1.5, 1 — как пришло с сервера речи. */
  volume: number;
  listen_on_start: boolean;
}

// ───────────── ответы команд экранов (src-tauri/src/api.rs; `demo` всегда false в приложении) ─────────────

export interface ProviderPreset {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string;
  key_url: string;
  docs_url: string;
  needs_key: boolean;
  /** Локальный сервер (Ollama, LM Studio). */
  local: boolean;
  /** Можно начать бесплатно. */
  free_tier: boolean;
  /** Короткое описание для карточки. */
  tagline: string;
  /** Технические заметки разработчика — только в свёрнутом «Подробнее». */
  note: string;
}

export interface ModelView {
  id: string;
  ctx: number;
  tools: boolean;
  vision: boolean;
  reasoning: boolean;
  latency_ms: number | null;
  tokens_per_s: number | null;
  tools_ok: boolean | null;
  vision_ok: boolean | null;
  quality: number;
  recommended_for: RoleName[];
}

export interface RoleRanking {
  role: RoleName;
  top: { model: string; reason: string }[];
}

export interface ProviderCheck {
  demo: boolean;
  status: 'ok' | 'bad_key' | 'network';
  message: string;
  models: ModelView[];
  ranking: RoleRanking[];
}

export interface ServiceCheck {
  demo: boolean;
  ok: boolean;
  message: string;
  latency_ms: number | null;
}

export interface SearchPreset {
  id: string;
  name: string;
  needs_key: boolean;
  extra_label: string;
  key_url: string;
}

export interface AudioDevice {
  id: string;
  name: string;
  default: boolean;
}
export interface AudioDevices {
  demo: boolean;
  inputs: AudioDevice[];
  outputs: AudioDevice[];
}

export interface Fact {
  id: string;
  text: string;
  confidence: number;
  source: string;
  updated: string;
}
export interface Lesson {
  id: string;
  trigger: string;
  what_went_wrong: string;
  fix: string;
  tool: string;
  seen: number;
  last_seen: string;
}
export interface Playbook {
  id: string;
  name: string;
  when: string;
  steps: string[];
  successes: number;
}
export interface MemoryData {
  demo: boolean;
  facts: Fact[];
  lessons: Lesson[];
  playbooks: Playbook[];
}
export type MemoryKind = 'facts' | 'lessons' | 'playbooks';

export interface KbHit {
  title: string;
  section: string;
  url: string;
  snippet: string;
  score: number;
}
export interface KbResult {
  demo: boolean;
  hits: KbHit[];
}

export interface HistoryItem {
  id: string;
  at: string;
  speaker: Speaker;
  text: string;
}
export interface HistoryResult {
  demo: boolean;
  items: HistoryItem[];
}

export interface OverlayGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
}

/** Микрофон (команды `mic_get` / `mic_set`). */
export interface MicStatus {
  enabled: boolean;
  listening: boolean;
  voice_ready: boolean;
  error: string | null;
}

/** Сводка ядра для HUD (`assistant_status`). */
export interface AssistantStatus {
  llm_ready: boolean;
  llm_problem: string | null;
  brain_model: string | null;
  mic: MicStatus;
  issues: string[];
  tools: number;
  busy: boolean;
}

export interface AppInfo {
  version: string;
  config_path: string;
  debug_build: boolean;
  elevated: boolean;
}

// Заглушки команд для режима «без Tauri» (обычный браузер). Только для разработки интерфейса:
// в приложении все команды обслуживает Rust (src-tauri/src/commands.rs).

import { playDemo } from './simulator';
import { EVENT_NAME, MIC_TEST_EVENT, type Config, type JarvisEvent } from './types';

type Handler = (payload: unknown) => void;

/** Локальная шина событий; BroadcastChannel синхронизирует вкладки (/hud и /overlay в разных вкладках). */
class BrowserBus {
  private handlers = new Map<string, Set<Handler>>();
  private channel: BroadcastChannel | null = null;

  private ensureChannel() {
    if (this.channel || typeof BroadcastChannel === 'undefined') return;
    this.channel = new BroadcastChannel('jarvis-dev-bus');
    this.channel.onmessage = (m: MessageEvent<{ name: string; payload: unknown }>) => this.deliver(m.data.name, m.data.payload);
  }

  subscribe(name: string, h: Handler): () => void {
    this.ensureChannel();
    if (!this.handlers.has(name)) this.handlers.set(name, new Set());
    this.handlers.get(name)!.add(h);
    return () => this.handlers.get(name)?.delete(h);
  }

  emit(name: string, payload: unknown) {
    this.ensureChannel();
    this.deliver(name, payload);
    this.channel?.postMessage({ name, payload });
  }

  private deliver(name: string, payload: unknown) {
    this.handlers.get(name)?.forEach((h) => h(payload));
  }
}

export const browserBus = new BrowserBus();
const emitEvent = (e: JarvisEvent) => browserBus.emit(EVENT_NAME, e);

let stopDemo: (() => void) | null = null;

export function defaultConfig(): Config {
  return {
    data_dir: 'data',
    tts: { url: 'https://192.168.50.38:8443', voice: 'jarvis-ru', ca_path: 'src-tauri/config/xeon-tts-ca.pem' },
    stt: { engine: 'server', url: 'https://192.168.50.38:8444', ca_path: 'src-tauri/config/xeon-tts-ca.pem' },
    providers: [{ id: 'groq', kind: 'openai_compat', base_url: 'https://api.groq.com/openai/v1', api_key: '', enabled: true }],
    roles: { brain: ['groq/openai/gpt-oss-120b'], fast: [], vision: [], reflection: [] },
    web_search: [{ id: 'duckduckgo', api_key: '', extra: '', enabled: true }],
    autonomy: 'ask_dangerous',
    run_as_admin: true,
    vision_enabled: true,
    terminal_autorun: true,
    resource_guard: true,
    search_roots: ['%USERPROFILE%', 'D:\\'],
    hud: { mode: 'corner', corner_x: null, corner_y: null },
    audio: { input_device: null, output_device: null, volume: 1, listen_on_start: true },
  };
}

/** Та же маска, что `jarvis_core::config::mask_key`: `gsk…abcd`. */
export function maskKey(key: string): string {
  const chars = Array.from(key);
  if (chars.length === 0) return '';
  if (chars.length <= 8) return '•'.repeat(chars.length);
  const head: string[] = [];
  for (const c of chars) {
    if (c === '_' || c === '-' || head.length >= 4) break;
    head.push(c);
  }
  const tail = chars.slice(-4).join('');
  return `${head.length < Math.floor(chars.length / 2) ? head.join('') : ''}…${tail}`;
}

let storedConfig: Config | null = null;
function loadConfig(): Config {
  if (storedConfig) return storedConfig;
  try {
    const raw = typeof localStorage !== 'undefined' ? localStorage.getItem('jarvis-dev-config') : null;
    storedConfig = raw ? { ...defaultConfig(), ...(JSON.parse(raw) as Partial<Config>) } : defaultConfig();
  } catch {
    storedConfig = defaultConfig();
  }
  return storedConfig;
}

function masked(c: Config): Config {
  return {
    ...c,
    providers: c.providers.map((p) => ({ ...p, api_key: maskKey(p.api_key) })),
    web_search: c.web_search.map((s) => ({ ...s, api_key: maskKey(s.api_key) })),
  };
}

const looksMasked = (s: string) => s.length > 0 && (s.includes('…') || Array.from(s).every((c) => c === '•'));
function mergeKey(incoming: string, old: string | undefined): string {
  if (old !== undefined && (incoming === maskKey(old) || looksMasked(incoming))) return old;
  if (old === undefined && looksMasked(incoming)) return '';
  return incoming.trim();
}

const memory = {
  demo: true,
  facts: [
    { id: 'f1', text: 'Пользователь пьёт чёрный кофе', confidence: 0.95, source: 'разговор', updated: '2026-11-20' },
    { id: 'f2', text: 'Рабочая папка проектов — D:\\projects', confidence: 0.8, source: 'file_search', updated: '2026-11-18' },
  ],
  lessons: [
    {
      id: 'l1',
      trigger: "git: 'comit' is not a git command",
      what_went_wrong: 'Опечатка в подкоманде git',
      fix: 'Заменить comit на commit',
      tool: 'terminal',
      seen: 3,
      last_seen: '2026-11-21',
    },
  ],
  playbooks: [
    {
      id: 'p1',
      name: 'Открыть настройки Bluetooth',
      when: '«открой блютуз», «настройки Bluetooth»',
      steps: ['open_target ms-settings:bluetooth', 'проверить заголовок окна «Bluetooth»'],
      successes: 5,
    },
  ],
};
let nextId = 100;
let mockMicTest: (() => void) | null = null;
let mockMic = { enabled: false, listening: false, voice_ready: false, error: 'в браузере микрофон не используется' as string | null };

export async function mockInvoke<T>(cmd: string, a: Record<string, unknown>): Promise<T> {
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case 'set_hud_mode':
      window.location.href = a.mode === 'fullscreen' ? '/hud/' : '/corner/';
      return r(null);
    case 'toggle_hud_mode':
      window.location.href = window.location.pathname.startsWith('/hud') ? '/corner/' : '/hud/';
      return r(null);
    case 'get_hud_mode':
      return r(window.location.pathname.startsWith('/hud') ? 'fullscreen' : 'corner');
    case 'open_settings':
      window.location.href = `/settings/${a.tab ? `#${String(a.tab)}` : ''}`;
      return r(null);
    case 'overlay_geometry':
      return r({ x: 0, y: 0, width: window.innerWidth * devicePixelRatio, height: window.innerHeight * devicePixelRatio, scale: devicePixelRatio });
    case 'config_get':
      return r(masked(loadConfig()));
    case 'config_set': {
      const old = loadConfig();
      const inc = a.config as Config;
      const merged: Config = {
        ...inc,
        providers: inc.providers.map((p) => ({ ...p, api_key: mergeKey(p.api_key, old.providers.find((o) => o.id === p.id)?.api_key) })),
        web_search: inc.web_search.map((s) => ({ ...s, api_key: mergeKey(s.api_key, old.web_search.find((o) => o.id === s.id)?.api_key) })),
      };
      storedConfig = merged;
      try {
        localStorage.setItem('jarvis-dev-config', JSON.stringify(merged));
      } catch {
        /* приватный режим — живём в памяти */
      }
      return r(masked(merged));
    }
    case 'config_path_get':
      return r('(браузер) localStorage: jarvis-dev-config');
    case 'app_info':
      return r({ version: '0.1.0', config_path: '(браузер)', debug_build: true, elevated: false });
    case 'emergency_stop':
      stopDemo?.();
      stopDemo = null;
      emitEvent({ type: 'control_stop' });
      emitEvent({ type: 'state', state: 'idle' });
      browserBus.emit('jarvis://emergency_stop', { source: 'button' });
      return r(null);
    case 'confirm_respond':
      emitEvent({ type: 'confirm_resolved', id: String(a.id), approved: Boolean(a.approved) });
      return r(null);
    case 'events_replay':
      return r([]);
    case 'open_url':
      if (/^https?:\/\//.test(String(a.url))) window.open(String(a.url), '_blank', 'noopener');
      return r(null);
    case 'simulate_demo':
      stopDemo?.();
      stopDemo = playDemo(emitEvent);
      return r(32000);
    case 'simulate_stop':
      stopDemo?.();
      stopDemo = null;
      return r(null);
    case 'simulate_event':
      emitEvent(a.event as JarvisEvent);
      return r(null);
    case 'agent_submit': {
      const text = String(a.text ?? '').trim();
      if (!text) throw 'Пустая команда';
      emitEvent({ type: 'subtitle', speaker: 'user', text, is_final: true });
      emitEvent({ type: 'subtitle', speaker: 'jarvis', text: 'В браузере я только интерфейс, сэр: запустите приложение.', is_final: true });
      return r(null);
    }
    case 'mic_get':
      return r(mockMic);
    case 'mic_set':
      mockMic = { ...mockMic, enabled: Boolean(a.enabled) };
      return r(mockMic);
    case 'assistant_status':
      return r({ llm_ready: true, llm_problem: null, brain_model: 'mock/demo', mic: mockMic, issues: [], tools: 0, busy: false });
    case 'provider_apply_best':
      return r(masked(a.config as Config));
    case 'provider_presets':
      return r([
        { id: 'groq', name: 'Groq', kind: 'openai_compat', base_url: 'https://api.groq.com/openai/v1', key_url: 'https://console.groq.com/keys', docs_url: '', needs_key: true, local: false, free_tier: true, tagline: 'Очень быстрые ответы, есть бесплатный ключ', note: 'GET /models отдаёт context_window.' },
        { id: 'opencode-zen', name: 'OpenCode Zen', kind: 'openai_compat', base_url: 'https://opencode.ai/inference/openai/v1', key_url: 'https://opencode.ai/console', docs_url: '', needs_key: true, local: false, free_tier: false, tagline: 'Подборка сильных моделей (Claude, GPT, Qwen) по одному ключу', note: '' },
        { id: 'openrouter', name: 'OpenRouter', kind: 'openai_compat', base_url: 'https://openrouter.ai/api/v1', key_url: 'https://openrouter.ai/keys', docs_url: '', needs_key: true, local: false, free_tier: true, tagline: 'Сотни моделей через один ключ, есть бесплатные', note: '' },
        { id: 'anthropic', name: 'Anthropic (Claude)', kind: 'anthropic', base_url: 'https://api.anthropic.com/v1', key_url: 'https://platform.claude.com/settings/keys', docs_url: '', needs_key: true, local: false, free_tier: false, tagline: 'Claude — сильный в рассуждениях и коде', note: '' },
        { id: 'ollama', name: 'Ollama (локально)', kind: 'openai_compat', base_url: 'http://localhost:11434/v1', key_url: '', docs_url: '', needs_key: false, local: true, free_tier: true, tagline: 'Модели на вашем ПК, без ключа и интернета', note: '' },
        { id: 'custom', name: 'Свой OpenAI-совместимый URL', kind: 'openai_compat', base_url: '', key_url: '', docs_url: '', needs_key: false, local: false, free_tier: false, tagline: 'Свой сервер: vLLM, llama.cpp, прокси', note: '' },
      ]);
    case 'provider_check': {
      const key = String(a.apiKey ?? '');
      const id = String(a.providerId);
      if (id !== 'ollama' && key.trim().length < 12 && !looksMasked(key))
        return r({ demo: true, status: 'bad_key', message: 'Ключ не принят провайдером (HTTP 401)', models: [], ranking: [] });
      const m = (mid: string, ctx: number, tools: boolean, vision: boolean, lat: number, tps: number, q: number, rec: string[]) => ({
        id: mid, ctx, tools, vision, reasoning: false, latency_ms: lat, tokens_per_s: tps, tools_ok: tools, vision_ok: vision ? true : null, quality: q, recommended_for: rec,
      });
      return r({
        demo: true,
        status: 'ok',
        message: 'Ключ рабочий · моделей: 3 · проверено 3',
        models: [
          m('openai/gpt-oss-120b', 131072, true, false, 380, 480, 4, ['brain', 'reflection']),
          m('openai/gpt-oss-20b', 131072, true, false, 210, 900, 3, ['fast']),
          m('meta-llama/llama-4-scout-17b-16e-instruct', 131072, true, true, 300, 700, 3, ['vision']),
        ],
        ranking: [
          { role: 'brain', top: [{ model: `${id}/openai/gpt-oss-120b`, reason: 'tools ✓ · русский ✓ · 0,4 с · 128k' }] },
          { role: 'fast', top: [{ model: `${id}/openai/gpt-oss-20b`, reason: 'tools ✓ · русский ✓ · 0,2 с · 128k' }] },
          { role: 'vision', top: [{ model: `${id}/meta-llama/llama-4-scout-17b-16e-instruct`, reason: 'vision ✓ · 0,3 с · 128k' }] },
          { role: 'reflection', top: [{ model: `${id}/openai/gpt-oss-120b`, reason: '128k' }] },
        ],
      });
    }
    case 'voice_check': {
      const ok = /^https?:\/\//.test(String(a.url));
      return r({ demo: true, ok, message: ok ? 'Сервер отвечает (демо)' : 'Адрес должен начинаться с https://', latency_ms: ok ? 40 : null });
    }
    case 'search_presets':
      return r([
        { id: 'brave', name: 'Brave Search', needs_key: true, extra_label: '', key_url: '' },
        { id: 'tavily', name: 'Tavily', needs_key: true, extra_label: '', key_url: '' },
        { id: 'google_cse', name: 'Google Programmable Search', needs_key: true, extra_label: 'Идентификатор поисковой системы (cx)', key_url: '' },
        { id: 'duckduckgo', name: 'DuckDuckGo (без ключа, запасной)', needs_key: false, extra_label: '', key_url: '' },
      ]);
    case 'search_check':
      return r({ demo: true, ok: a.id === 'duckduckgo' || String(a.apiKey ?? '').length > 0, message: 'Работает ✓ (8 результатов, 0,6 с)', latency_ms: 600 });
    case 'audio_devices':
      return r({
        demo: true,
        inputs: [
          { id: 'Микрофон (Realtek Audio)', name: 'Микрофон (Realtek Audio)', default: true },
          { id: 'Гарнитура (USB Audio)', name: 'Гарнитура (USB Audio)', default: false },
        ],
        outputs: [
          { id: 'Динамики (Realtek Audio)', name: 'Динамики (Realtek Audio)', default: true },
          { id: 'Наушники (USB Audio)', name: 'Наушники (USB Audio)', default: false },
        ],
      });
    case 'audio_mic_test': {
      // в браузере — синтетический уровень, чтобы видеть индикатор
      const secs = Number(a.seconds ?? 10);
      mockMicTest?.();
      const t0 = Date.now();
      const timer = setInterval(() => browserBus.emit(MIC_TEST_EVENT, Math.abs(Math.sin((Date.now() - t0) / 300)) * 0.25 * Math.random()), 33);
      return new Promise<T>((resolve) => {
        const done = () => {
          clearInterval(timer);
          clearTimeout(to);
          mockMicTest = null;
          resolve(r(String(a.device ?? '') || 'Микрофон (Realtek Audio)'));
        };
        const to = setTimeout(done, secs * 1000);
        mockMicTest = done;
      });
    }
    case 'audio_mic_test_stop':
      mockMicTest?.();
      return r(null);
    case 'audio_sound_test':
      return r({ demo: true, ok: true, message: 'Джарвис сказал тестовую фразу (демо).', latency_ms: 900 });
    case 'memory_list':
      return r(structuredClone(memory));
    case 'memory_upsert': {
      const kind = a.kind as 'facts' | 'lessons' | 'playbooks';
      const item = { ...(a.item as { id?: string }) } as { id: string };
      if (!item.id) item.id = `${kind[0]}${nextId++}`;
      const list = memory[kind] as { id: string }[];
      const i = list.findIndex((x) => x.id === item.id);
      if (i >= 0) list[i] = item;
      else list.push(item);
      return r(structuredClone(memory));
    }
    case 'memory_delete': {
      const kind = a.kind as 'facts' | 'lessons' | 'playbooks';
      const list = memory[kind] as { id: string }[];
      const i = list.findIndex((x) => x.id === a.id);
      if (i >= 0) list.splice(i, 1);
      return r(structuredClone(memory));
    }
    case 'kb_search':
      return r({
        demo: true,
        hits: String(a.query ?? '').trim()
          ? [
              {
                title: 'Stop-Service',
                section: 'PowerShell › Microsoft.PowerShell.Management',
                url: 'https://learn.microsoft.com/powershell/module/microsoft.powershell.management/stop-service',
                snippet: 'Останавливает одну или несколько запущенных служб.',
                score: 1,
              },
            ]
          : [],
      });
    case 'history_list':
      return r({
        demo: true,
        items: [
          { id: 'h1', at: '2026-11-22T09:14:03', speaker: 'user', text: 'Джарвис, что сейчас жрёт память?' },
          { id: 'h2', at: '2026-11-22T09:14:05', speaker: 'jarvis', text: 'Сэр, память занята на 71 %, больше всего берёт браузер.' },
        ],
      });
    default:
      throw `Команда «${cmd}» недоступна вне Tauri`;
  }
}

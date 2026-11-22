'use client';

import { useEffect, useState } from 'react';

import Reactor from '@/components/reactor/Reactor';
import { STATE_LABEL } from '@/lib/glow';
import { jarvisStore } from '@/lib/store';
import { DEFAULT_APPEARANCE, THEMES, contrast, isHex, normalizeAppearance, paletteFor, signalClash, themeById } from '@/lib/theme';
import type { AppearanceConfig, AssistantState, CornerSize } from '@/lib/types';

import { Field, Toggle, type TabProps } from './common';

const PREVIEW_STATES: AssistantState[] = ['idle', 'listening', 'thinking', 'speaking', 'controlling'];

const SIZES: { v: CornerSize; name: string; px: number }[] = [
  { v: 'small', name: 'S', px: 160 },
  { v: 'medium', name: 'M', px: 220 },
  { v: 'large', name: 'L', px: 300 },
];

/** Настройки → «Внешний вид»: тема, свой акцент, свечение, анимация, виджет, панели HUD. Предпросмотр — живой. */
export default function AppearanceTab({ draft, update }: TabProps) {
  const ap = normalizeAppearance(draft.appearance);
  const set = (patch: Partial<AppearanceConfig>) => update((c) => ({ ...c, appearance: { ...normalizeAppearance(c.appearance), ...patch } }));
  const pal = paletteFor(ap);
  const theme = themeById(ap.theme);
  const [preview, setPreview] = useState<AssistantState>('idle');
  const [hexInput, setHexInput] = useState(ap.accent ?? '');

  useEffect(() => setHexInput(ap.accent ?? ''), [ap.accent]);
  useEffect(() => {
    jarvisStore.dispatch({ type: 'state', state: preview });
  }, [preview]);
  useEffect(() => () => jarvisStore.dispatch({ type: 'state', state: 'idle' }), []);

  const ratio = contrast(pal.accentText, pal.void);
  const lightened = pal.accentText !== pal.accent;
  const clash = signalClash(pal.accent);

  return (
    <>
      <section className="section">
        <h2 className="section-title">Тема</h2>
        <p className="section-lead">
          Цвет меняется во всех окнах: HUD, виджет, рамка управления ПК, настройки. Янтарь (опасное действие) и красный (подтверждение,
          ошибка) — сигнальные и не перекрашиваются.
        </p>
        <div className="appearance">
          <div className="theme-grid" role="radiogroup" aria-label="Тема">
            {THEMES.map((t) => (
              <button
                key={t.id}
                type="button"
                role="radio"
                aria-checked={ap.theme === t.id && !ap.accent}
                className="theme-tile"
                data-active={ap.theme === t.id}
                onClick={() => set({ theme: t.id, accent: null })}
                style={{ ['--t-accent' as string]: t.accent, ['--t-void' as string]: t.void, ['--t-deep' as string]: t.deep, ['--t-steel' as string]: t.steel, ['--t-ice' as string]: t.ice }}
              >
                <span className="theme-swatch" aria-hidden="true">
                  <span className="theme-ring" />
                </span>
                <span className="theme-name">{t.name}</span>
                <span className="theme-desc">{t.desc}</span>
              </button>
            ))}
          </div>

          <figure className="preview panel" aria-label="Предпросмотр">
            <Reactor variant="corner" className="reactor reactor-preview" />
            <figcaption className="preview-states">
              {PREVIEW_STATES.map((s) => (
                <button key={s} type="button" className="chip" aria-pressed={preview === s} onClick={() => setPreview(s)}>
                  {STATE_LABEL[s]}
                </button>
              ))}
            </figcaption>
          </figure>
        </div>

        <div className="card accent-card">
          <div className="card-head">
            <span className="card-title">Свой акцентный цвет</span>
            <span className="muted small">{ap.accent ? `поверх темы «${theme.name}»` : `сейчас — цвет темы «${theme.name}»`}</span>
          </div>
          <div className="row">
            <input
              type="color"
              className="color-input"
              aria-label="Акцентный цвет"
              value={(ap.accent ?? theme.accent).toLowerCase()}
              onChange={(e) => set({ accent: e.target.value.toUpperCase() })}
            />
            <input
              className="input mono hex-input"
              value={hexInput}
              placeholder={theme.accent}
              spellCheck={false}
              maxLength={7}
              aria-label="Цвет в формате #RRGGBB"
              onChange={(e) => {
                const v = e.target.value.trim();
                setHexInput(v);
                if (isHex(v)) set({ accent: v.toUpperCase() });
              }}
            />
            <button type="button" className="btn btn-ghost" disabled={!ap.accent} onClick={() => set({ accent: null })}>
              Цвет темы
            </button>
          </div>
          <p className="small" data-ok={ratio >= 4.5}>
            Контраст текста акцентом на фоне: <strong>{ratio.toFixed(1)} : 1</strong> {ratio >= 7 ? '— отлично' : ratio >= 4.5 ? '— хорошо' : '— мало'}
            {lightened && <span className="muted"> · цвет тёмный, для текста и рамок он осветлён до {pal.accentText}</span>}
          </p>
          {clash && (
            <p className="status small">
              Этот цвет похож на сигнальный {clash === 'amber' ? 'янтарь (опасное действие)' : 'красный (подтверждение и ошибки)'} — поэтому рамка
              обычных действий на экране останется циановой, чтобы их нельзя было перепутать.
            </p>
          )}
        </div>
      </section>

      <section className="section">
        <h2 className="section-title">Свечение и анимация</h2>
        <Field label={`Яркость свечения: ${Math.round(ap.glow * 100)} %`}>
          <input className="range" type="range" min={30} max={150} step={5} value={Math.round(ap.glow * 100)} onChange={(e) => set({ glow: Number(e.target.value) / 100 })} />
        </Field>
        <Field label={`Скорость анимации реактора: ${Math.round(ap.animation_speed * 100)} %`}>
          <input
            className="range"
            type="range"
            min={25}
            max={200}
            step={5}
            value={Math.round(ap.animation_speed * 100)}
            onChange={(e) => set({ animation_speed: Number(e.target.value) / 100 })}
          />
        </Field>
        <Toggle
          checked={ap.scanlines}
          onChange={(v) => set({ scanlines: v })}
          title="Эффект плёнки в HUD"
          desc="Гекс-сетка и бегущие линии сканирования на фоне. Выключите, если отвлекает."
        />
      </section>

      <section className="section">
        <h2 className="section-title">Виджет в углу</h2>
        <div className="segmented" role="radiogroup" aria-label="Размер виджета">
          {SIZES.map((s) => (
            <button key={s.v} type="button" role="radio" aria-checked={ap.corner_size === s.v} className="segment" onClick={() => set({ corner_size: s.v })}>
              <span className="segment-name">{s.name}</span>
              <span className="muted small">{s.px} px</span>
            </button>
          ))}
        </div>
        <p className="muted small">Правый нижний угол виджета остаётся на месте.</p>
      </section>

      <section className="section">
        <h2 className="section-title">Панели HUD</h2>
        <Toggle checked={ap.show_telemetry} onChange={(v) => set({ show_telemetry: v })} title="Телеметрия" desc="Процессор, память, диск слева." />
        <Toggle checked={ap.show_actions} onChange={(v) => set({ show_actions: v })} title="Лента действий" desc="Шаги, которые Джарвис выполняет на ПК." />
        <Toggle checked={ap.show_sources} onChange={(v) => set({ show_sources: v })} title="Источники" desc="Ссылки, по которым Джарвис нашёл ответ." />
        <Toggle checked={ap.show_subtitles} onChange={(v) => set({ show_subtitles: v })} title="Субтитры" desc="Что вы сказали и что отвечает Джарвис." />
      </section>

      <div className="row">
        <button
          type="button"
          className="btn"
          disabled={JSON.stringify(ap) === JSON.stringify(DEFAULT_APPEARANCE)}
          onClick={() => update((c) => ({ ...c, appearance: { ...DEFAULT_APPEARANCE } }))}
        >
          Сбросить внешний вид по умолчанию
        </button>
        <span className="muted small">Изменения видны здесь сразу; в остальных окнах — после «Сохранить».</span>
      </div>
    </>
  );
}

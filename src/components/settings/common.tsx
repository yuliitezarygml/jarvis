'use client';

import { useState } from 'react';

import { call } from '@/lib/bridge';
import type { Config, ServiceCheck } from '@/lib/types';

export interface TabProps {
  draft: Config;
  update: (f: (c: Config) => Config) => void;
}

export function Toggle({
  checked,
  onChange,
  title,
  desc,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  title: string;
  desc?: string;
}) {
  return (
    <label className="toggle">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="toggle-text">
        <span>{title}</span>
        {desc && <span className="toggle-desc">{desc}</span>}
      </span>
    </label>
  );
}

export function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
    </label>
  );
}

/** Выключатель «вкл/выкл» — крупный и с подписью, чтобы состояние было видно сразу (не крошечный чекбокс). */
export function Switch({
  checked,
  onChange,
  onText = 'Включён',
  offText = 'Выключен',
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  onText?: string;
  offText?: string;
  /** Для экранного диктора: что включается. */
  label: string;
}) {
  return (
    <button type="button" role="switch" aria-checked={checked} aria-label={label} className="switch" onClick={() => onChange(!checked)}>
      <span className="switch-track" aria-hidden="true">
        <span className="switch-thumb" />
      </span>
      <span className="switch-text">{checked ? onText : offText}</span>
    </button>
  );
}

const isMask = (v: string) => v.includes('…') || /^•+$/.test(v);

/** Поле нового ключа со «Показать/Скрыть» (сохранённый ключ в UI не приходит — только маска). */
export function SecretInput({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  const [shown, setShown] = useState(false);
  return (
    <div className="secret">
      <input
        className="input"
        type={shown ? 'text' : 'password'}
        autoComplete="off"
        spellCheck={false}
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
      <button
        type="button"
        className="secret-eye"
        aria-label={shown ? 'Скрыть ключ' : 'Показать ключ'}
        aria-pressed={shown}
        disabled={!value}
        onClick={() => setShown((s) => !s)}
      >
        {shown ? 'Скрыть' : 'Показать'}
      </button>
    </div>
  );
}

/** Поле ключа: маска сохранённого ключа + отдельное поле для нового (с «Показать»). */
export function KeyField({ value, onChange, label = 'Ключ API' }: { value: string; onChange: (v: string) => void; label?: string }) {
  const [typed, setTyped] = useState('');
  // маска сохранённого ключа на момент монтирования (после сохранения родитель перемонтирует поле по key)
  const [initial, setInitial] = useState(value);
  const masked = isMask(initial);
  return (
    <div className="field">
      <span className="field-label">
        {label}
        {masked && !typed && (
          <span className="key-saved">
            {' '}
            · сохранён <span className="mono">{initial}</span>
          </span>
        )}
        {!masked && !typed && !value && <span className="key-missing"> · не задан</span>}
      </span>
      <div className="row">
        <div className="grow">
          <SecretInput
            value={typed}
            placeholder={masked ? 'вставьте новый, чтобы заменить' : 'вставьте ключ'}
            onChange={(v) => {
              setTyped(v);
              onChange(v.trim() ? v : masked ? initial : '');
            }}
          />
        </div>
        {(masked || value) && (
          <button
            type="button"
            className="btn btn-small btn-ghost"
            onClick={() => {
              setTyped('');
              setInitial('');
              onChange('');
            }}
          >
            Удалить ключ
          </button>
        )}
      </div>
    </div>
  );
}

/** Ссылка во внешний браузер (через `open_url`, в окне Tauri обычные ссылки не открываются). */
export function ExternalLink({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <a
      className="ext-link"
      href={href}
      onClick={(e) => {
        e.preventDefault();
        void call('open_url', { url: href });
      }}
    >
      {children} <span aria-hidden="true">↗</span>
    </a>
  );
}

export function CheckStatus({ check }: { check: ServiceCheck | 'loading' | undefined }) {
  if (!check) return null;
  if (check === 'loading') return <p className="status">Проверяю…</p>;
  return (
    <p className="status" data-ok={check.ok}>
      {check.message}
      {check.demo && <span className="muted"> · демо</span>}
    </p>
  );
}

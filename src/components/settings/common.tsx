'use client';

import { useState } from 'react';

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

/** Поле ключа: показывает маску сохранённого ключа, новый ключ вводится в отдельное скрытое поле. */
export function KeyField({ value, onChange, label = 'Ключ API' }: { value: string; onChange: (v: string) => void; label?: string }) {
  const [typed, setTyped] = useState('');
  // маска сохранённого ключа на момент монтирования (после сохранения родитель перемонтирует поле по key)
  const [initial, setInitial] = useState(value);
  const isMask = (v: string) => v.includes('…') || /^•+$/.test(v);
  const masked = isMask(initial);
  return (
    <Field label={label}>
      <div className="row">
        <input
          className="input grow"
          type="password"
          autoComplete="off"
          spellCheck={false}
          placeholder={isMask(value) ? `сохранён: ${value} — вставьте новый, чтобы заменить` : value ? 'новый ключ введён' : 'вставьте ключ'}
          value={typed}
          onChange={(e) => {
            setTyped(e.target.value);
            onChange(e.target.value.trim() ? e.target.value : isMask(initial) ? initial : '');
          }}
        />
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
    </Field>
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

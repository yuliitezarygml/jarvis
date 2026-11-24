'use client';

import { useState } from 'react';

import { PERSONA_MODES, PERSONA_VOICE_COMMANDS, TEASE_LEVELS, addBannedWords, addressPlaceholder, personaTitle, removeBannedWord } from '@/lib/persona';
import type { Config, PersonaConfig } from '@/lib/types';

import { Field, Toggle, type TabProps } from './common';

const setPersona = (c: Config, patch: Partial<PersonaConfig>): Config => ({ ...c, persona: { ...c.persona, ...patch } });

/** «Никогда не говори»: слова-чипы + поле ввода (Enter или запятая — добавить). */
function BannedWords({ words, onChange }: { words: string[]; onChange: (w: string[]) => void }) {
  const [input, setInput] = useState('');
  const commit = () => {
    if (!input.trim()) return;
    onChange(addBannedWords(words, input));
    setInput('');
  };
  return (
    <div className="banned">
      {words.length > 0 && (
        <ul className="banned-list" aria-label="Запретные слова">
          {words.map((w) => (
            <li key={w} className="banned-chip">
              <span>{w}</span>
              <button type="button" className="banned-remove" aria-label={`Убрать «${w}»`} onClick={() => onChange(removeBannedWord(words, w))}>
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="row">
        <input
          className="input grow"
          value={input}
          placeholder="слово или несколько через запятую"
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              commit();
            }
          }}
        />
        <button type="button" className="btn" disabled={!input.trim()} onClick={commit}>
          Добавить
        </button>
      </div>
    </div>
  );
}

export default function PersonaTab({ draft, update }: TabProps) {
  const p = draft.persona;
  return (
    <>
      <section className="section">
        <h2 className="section-title">Режим</h2>
        <p className="section-lead">Как Джарвис разговаривает с вами. Задачи он выполняет одинаково в любом режиме, а опасные действия делает только после «да».</p>
        <div className="persona-modes" role="radiogroup" aria-label="Режим характера">
          {PERSONA_MODES.map((m) => (
            <label className="persona-card" key={m.v} data-checked={p.mode === m.v ? '' : undefined}>
              <input type="radio" name="persona-mode" checked={p.mode === m.v} onChange={() => update((c) => setPersona(c, { mode: m.v }))} />
              <span className="toggle-text">
                <span className="persona-title">{m.title}</span>
                <span className="toggle-desc">{m.desc}</span>
                {m.warn && <span className="persona-warn">{m.warn}</span>}
              </span>
            </label>
          ))}
        </div>
        <p className="persona-hint">
          Голосом:{' '}
          {PERSONA_VOICE_COMMANDS.map((v, i) => (
            <span key={v.mode}>
              {i > 0 && ' · '}
              {v.phrase} → {personaTitle(v.mode)}
            </span>
          ))}
          . Переключить можно и из меню значка в трее.
        </p>
      </section>

      <section className="section">
        <h2 className="section-title">Манера</h2>
        <Field label="Как часто подкалывать">
          <div className="segmented" role="radiogroup" aria-label="Как часто подкалывать">
            {TEASE_LEVELS.map((t) => (
              <label key={t.v} className="segmented-item" data-checked={p.tease === t.v ? '' : undefined}>
                <input type="radio" name="persona-tease" checked={p.tease === t.v} onChange={() => update((c) => setPersona(c, { tease: t.v }))} />
                {t.title}
              </label>
            ))}
          </div>
        </Field>
        <Field label="Как обращаться к вам">
          <input className="input" value={p.address} placeholder={addressPlaceholder(p.mode)} onChange={(e) => update((c) => setPersona(c, { address: e.target.value }))} />
        </Field>
        <Toggle
          checked={p.adaptive}
          onChange={(v) => update((c) => setPersona(c, { adaptive: v }))}
          title="Подстраиваться под мой тон"
          desc="Замечает, когда вы кричите, торопитесь, материтесь или шутите, и отвечает в тон."
        />
        <Toggle
          checked={p.mood}
          onChange={(v) => update((c) => setPersona(c, { mood: v }))}
          title="Своё настроение (обижается)"
          desc="Если его несколько раз послать, пару минут отвечает суше. Проходит само или после извинения. Значок настроения виден в HUD."
        />
      </section>

      <section className="section">
        <h2 className="section-title">Никогда не говори</h2>
        <p className="section-lead">Слова, которых Джарвис не скажет ни в каком режиме.</p>
        <BannedWords words={p.banned_words} onChange={(w) => update((c) => setPersona(c, { banned_words: w }))} />
      </section>

      <section className="section">
        <h2 className="section-title">Словарь</h2>
        <p className="section-lead">
          Чтобы понимать вашу речь и отвечать своими словами, Джарвис берёт слова и фразы из папки <code className="rt-code">config/persona/</code> (обычные
          JSON-файлы, правки подхватываются без перезапуска). Ваши любимые словечки он замечает сам и дописывает в{' '}
          <code className="rt-code">lexicon/user.json</code>. Что он понял о вашей манере общения, видно в «Памяти»: раздел «Факты», фильтр «Характер».
        </p>
      </section>
    </>
  );
}

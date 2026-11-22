'use client';

import { useState } from 'react';

import AppShell, { DemoBadge } from '@/components/AppShell';
import { call, errorText } from '@/lib/bridge';
import type { KbResult } from '@/lib/types';

export default function KbPage() {
  const [query, setQuery] = useState('');
  const [result, setResult] = useState<KbResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const search = async () => {
    setBusy(true);
    try {
      setResult(await call<KbResult>('kb_search', { query }));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <AppShell>
      <div className="page-head">
        <h1 className="page-title">База знаний Windows</h1>
        <DemoBadge show={result?.demo} />
      </div>
      <p className="section-lead">
        Документация Microsoft (PowerShell, команды Windows, Sysinternals, Win32) и свои шпаргалки. Джарвис ищет здесь сам, прежде чем
        идти в интернет.
      </p>
      <form
        className="row"
        style={{ maxWidth: 760, marginBottom: 18 }}
        onSubmit={(e) => {
          e.preventDefault();
          void search();
        }}
      >
        <input
          className="input grow"
          type="search"
          value={query}
          placeholder="например: остановить службу, автозагрузка, сброс DNS"
          onChange={(e) => setQuery(e.target.value)}
          aria-label="Запрос"
        />
        <button type="submit" className="btn btn-primary" disabled={busy}>
          {busy ? 'Ищу…' : 'Найти'}
        </button>
      </form>
      {error && <p className="status" data-ok="false">{error}</p>}
      {result && result.hits.length === 0 && <p className="muted">Ничего не нашлось. Попробуйте английский термин: «service», «startup», «firewall».</p>}
      {result?.hits.map((h) => (
        <article key={h.url} className="kb-hit">
          <div className="kb-title">{h.title}</div>
          <div className="kb-section">{h.section}</div>
          <p className="mem-main">{h.snippet}</p>
          <div className="row">
            <span className="kb-url">{h.url}</span>
            <button type="button" className="btn btn-small btn-ghost" onClick={() => void call('open_url', { url: h.url })}>
              Открыть в браузере
            </button>
          </div>
        </article>
      ))}
    </AppShell>
  );
}

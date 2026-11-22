'use client';

import { call } from '@/lib/bridge';
import { useJarvis } from '@/lib/store';

function host(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

/** Источники последнего ответа из интернета. Номера совпадают с голосовой командой «открой второй». */
export default function SourcesPanel() {
  const items = useJarvis((s) => s.sources);
  if (items.length === 0) return null;
  return (
    <section className="panel sources" aria-label="Источники">
      <h2 className="panel-title">Источники</h2>
      <ol className="source-list">
        {items.map((s, i) => (
          <li key={`${i}-${s.url}`}>
            <button type="button" className="source" onClick={() => void call('open_url', { url: s.url })} title={s.url}>
              <span className="source-n">{i + 1}</span>
              <span className="source-text">
                <span className="source-title">{s.title}</span>
                <span className="source-host">{host(s.url)}</span>
              </span>
            </button>
          </li>
        ))}
      </ol>
    </section>
  );
}

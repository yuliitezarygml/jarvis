'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';

import { call } from '@/lib/bridge';

const NAV = [
  { href: '/settings/', name: 'Настройки' },
  { href: '/memory/', name: 'Память' },
  { href: '/kb/', name: 'База знаний' },
  { href: '/log/', name: 'Журнал' },
];

/** Каркас окна настроек: навигация слева, содержимое справа. */
export default function AppShell({ children }: { children: React.ReactNode }) {
  const path = usePathname() ?? '';
  const norm = path.endsWith('/') ? path : `${path}/`;
  return (
    <div className="shell">
      <nav className="shell-nav" aria-label="Разделы">
        <span className="wordmark">J.A.R.V.I.S.</span>
        {NAV.map((n) => (
          <Link key={n.href} href={n.href} className="shell-link" aria-current={norm === n.href ? 'page' : undefined}>
            {n.name}
          </Link>
        ))}
        <div className="shell-nav-foot">
          <button type="button" className="btn btn-small" onClick={() => void call('set_hud_mode', { mode: 'fullscreen' })}>
            Открыть HUD
          </button>
        </div>
      </nav>
      <main className="shell-main">{children}</main>
    </div>
  );
}

export function DemoBadge({ show }: { show: boolean | undefined }) {
  if (!show) return null;
  return (
    <span className="demo-badge" title="Бэкенд ещё не подключён: данные-заглушки для проверки интерфейса">
      демо-данные
    </span>
  );
}

import Link from 'next/link';

const SCREENS: { href: string; name: string; what: string }[] = [
  { href: '/hud/', name: 'HUD', what: 'Полноэкранный режим: реактор, телеметрия, действия, субтитры' },
  { href: '/corner/', name: 'Виджет', what: 'Компактный реактор для угла экрана' },
  { href: '/overlay/', name: 'Оверлей', what: 'Свечение по краю экрана во время управления ПК' },
  { href: '/settings/', name: 'Настройки', what: 'Провайдеры, голос, аудио, разрешения, окна, поиск' },
  { href: '/memory/', name: 'Память', what: 'Факты, уроки и плейбуки' },
  { href: '/kb/', name: 'База знаний', what: 'Поиск по документации Windows' },
  { href: '/log/', name: 'Журнал', what: 'История диалогов и лента действий' },
];

/** Оглавление экранов — нужно только при разработке в браузере; окна Tauri открывают свои адреса напрямую. */
export default function Index() {
  return (
    <main className="shell-plain">
      <h1 className="wordmark big">J.A.R.V.I.S.</h1>
      <p className="muted">Экраны интерфейса. В приложении каждый открывается в своём окне.</p>
      <ul className="index-list">
        {SCREENS.map((s) => (
          <li key={s.href}>
            <Link href={s.href}>{s.name}</Link>
            <span className="muted"> — {s.what}</span>
          </li>
        ))}
      </ul>
    </main>
  );
}

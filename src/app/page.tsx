import Link from "next/link";
import { RustDemo } from "@/components/rust-demo";

const stack = [
  { number: "01", title: "Next.js + React", detail: "Интерфейс и маршруты" },
  { number: "02", title: "Tauri 2", detail: "Нативное окно приложения" },
  { number: "03", title: "Rust", detail: "Команды и логика приложения" },
];

export default function Home() {
  return (
    <div className="app-shell">
      <header className="app-header">
        <Link className="brand" href="/" aria-label="Jarvis — главная">
          <span className="brand-icon" aria-hidden="true">j.</span>
          jarvis
        </Link>
        <span className="version">v0.1.0</span>
      </header>

      <main>
        <section className="intro" aria-labelledby="page-title">
          <span className="eyebrow"><span className="status-dot" /> ОСНОВА ПРИЛОЖЕНИЯ</span>
          <h1 id="page-title">Всё начинается<br />с хорошей основы<span className="accent">.</span></h1>
          <p className="intro-text">Интерфейс на React. Логика на Rust.<br />Одно desktop-приложение — готово к твоим идеям.</p>
        </section>

        <section className="stack" aria-label="Стек приложения">
          {stack.map((item) => (
            <article className="stack-item" key={item.number}>
              <span className="stack-number">{item.number}</span>
              <h2>{item.title}</h2>
              <p>{item.detail}</p>
            </article>
          ))}
        </section>

        <RustDemo />
      </main>

      <footer className="app-footer">
        <span>Твоё рабочее пространство.</span>
        <span>Rust / Tauri / Next.js</span>
      </footer>
    </div>
  );
}

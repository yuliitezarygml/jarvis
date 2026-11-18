"use client";

import { useActionState } from "react";
import { getAppInfo, greet, isTauri, type AppInfo } from "@/lib/tauri";

interface DemoState {
  message: string;
  info: AppInfo | null;
  error: boolean;
}

const initialState: DemoState = {
  message: "Введи имя и отправь первую команду в Rust.",
  info: null,
  error: false,
};

async function sendGreeting(
  _previousState: DemoState,
  formData: FormData,
): Promise<DemoState> {
  if (!isTauri()) {
    return {
      message: "Сейчас открыт веб-просмотр. Запусти desktop-приложение: npm run desktop:dev",
      info: null,
      error: true,
    };
  }

  try {
    const name = String(formData.get("name") ?? "");
    const [message, info] = await Promise.all([greet(name), getAppInfo()]);
    return { message, info, error: false };
  } catch (error) {
    return {
      message: error instanceof Error ? error.message : String(error),
      info: null,
      error: true,
    };
  }
}

export function RustDemo() {
  const [state, formAction, pending] = useActionState(sendGreeting, initialState);

  return (
    <section className="demo" aria-labelledby="demo-title">
      <div className="demo-heading">
        <div>
          <span className="eyebrow">ПЕРВАЯ КОМАНДА</span>
          <h2 id="demo-title">Познакомимся?</h2>
        </div>
        <span className="connection-path" aria-hidden="true">React <span>↔</span> Rust</span>
      </div>

      <form action={formAction} className="greeting-form" aria-busy={pending}>
        <div className="input-group">
          <label htmlFor="name">Как тебя зовут?</label>
          <input
            id="name"
            name="name"
            type="text"
            placeholder="Твоё имя"
            autoComplete="given-name"
            required
            maxLength={64}
          />
        </div>
        <button type="submit" disabled={pending}>
          {pending ? "Отправляем…" : "Проверить Rust"}
          <span aria-hidden="true">↗</span>
        </button>
      </form>

      <div className={`command-result${state.error ? " command-error" : ""}`} role="status" aria-live="polite" aria-atomic="true">
        <span className="result-prompt" aria-hidden="true">›</span>
        <div>
          <p>{state.message}</p>
          {state.info ? (
            <p className="runtime-info">{state.info.name} {state.info.version} · {state.info.os} / {state.info.arch}</p>
          ) : null}
        </div>
      </div>
    </section>
  );
}

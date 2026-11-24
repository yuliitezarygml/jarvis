'use client';

import { useEffect, useState } from 'react';

import DevPanel from '@/components/DevPanel';
import ActionFeed from '@/components/hud/ActionFeed';
import CommandBar from '@/components/hud/CommandBar';
import ConfirmDialog from '@/components/hud/ConfirmDialog';
import ErrorToast from '@/components/hud/ErrorToast';
import HudBackground from '@/components/hud/HudBackground';
import SourcesPanel from '@/components/hud/SourcesPanel';
import Subtitles from '@/components/hud/Subtitles';
import Telemetry from '@/components/hud/Telemetry';
import Waveform from '@/components/hud/Waveform';
import Reactor from '@/components/reactor/Reactor';
import { call } from '@/lib/bridge';
import { fmtClock } from '@/lib/format';
import { STATE_LABEL, TIER_LABEL } from '@/lib/glow';
import { useJarvis, useNow, useTauriEvent } from '@/lib/store';
import { EMERGENCY_EVENT } from '@/lib/types';

function Clock() {
  const now = useNow(1000);
  return <span className="hud-clock">{now ? fmtClock(new Date(now)) : '--:--:--'}</span>;
}

export default function HudPage() {
  const assistant = useJarvis((s) => s.assistant);
  const control = useJarvis((s) => s.control);
  const [stopped, setStopped] = useState<number | null>(null);

  useTauriEvent(EMERGENCY_EVENT, () => setStopped(Date.now()));

  const [fullscreen, setFullscreen] = useState(false);
  // `?wallpaper` — та же страница как живые обои рабочего стола: без кнопок и поля ввода (клики идут иконкам).
  const [wallpaper, setWallpaper] = useState(false);
  const [wallpaperOn, setWallpaperOn] = useState(false);
  useEffect(() => {
    const isWallpaper = new URLSearchParams(window.location.search).has('wallpaper');
    setWallpaper(isWallpaper);
    void call<boolean>('get_wallpaper_mode')
      .then((v) => setWallpaperOn(Boolean(v)))
      .catch(() => {});
  }, []);
  const toggleWallpaper = async () => {
    try {
      setWallpaperOn(Boolean(await call<boolean>('set_wallpaper_mode', { on: !wallpaperOn })));
    } catch {
      /* в браузере без Tauri обоев нет */
    }
  };

  const toggleFullscreen = async () => {
    try {
      setFullscreen(Boolean(await call<boolean>('toggle_hud_fullscreen')));
    } catch {
      /* в браузере без Tauri окна нет */
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'F11') {
        e.preventDefault();
        void toggleFullscreen();
        return;
      }
      if (e.key !== 'Escape' || document.querySelector('.confirm')) return;
      // Esc: сначала выход из полного экрана в обычное окно, повторный Esc — в угол.
      void (async () => {
        let wasFull = false;
        try {
          wasFull = Boolean(await call<boolean>('exit_hud_fullscreen'));
        } catch {
          /* нет Tauri */
        }
        if (wasFull) setFullscreen(false);
        else void call('set_hud_mode', { mode: 'corner' });
      })();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  useEffect(() => {
    if (stopped === null) return;
    const t = setTimeout(() => setStopped(null), 4000);
    return () => clearTimeout(t);
  }, [stopped]);

  return (
    <main className="hud" data-state={assistant} data-wallpaper={wallpaper ? '' : undefined}>
      <HudBackground />

      <header className="hud-top">
        <div className="hud-brand">
          <span className="wordmark">J.A.R.V.I.S.</span>
          <span className="hud-state" data-state={assistant}>
            <span className="state-dot" aria-hidden="true" />
            {STATE_LABEL[assistant]}
            {control.active && control.tier && <span className="hud-tier"> · {TIER_LABEL[control.tier]}</span>}
          </span>
        </div>
        <Clock />
        <nav className="hud-actions" aria-label="Управление окном">
          <button type="button" className={wallpaperOn ? 'btn btn-primary' : 'btn btn-ghost'} onClick={() => void toggleWallpaper()} title="HUD как обои рабочего стола">
            Обои
          </button>
          {wallpaper ? (
            <button type="button" className="btn btn-ghost" onClick={() => void call('set_hud_mode', { mode: 'fullscreen' })} title="Ctrl+Alt+J">
              Открыть окно
            </button>
          ) : (
            <>
              <button type="button" className="btn btn-ghost" onClick={() => void toggleFullscreen()} title="F11">
                {fullscreen ? 'Окно' : 'Во весь экран'}
              </button>
              <button type="button" className="btn btn-ghost" onClick={() => void call('set_hud_mode', { mode: 'corner' })} title="Esc или Ctrl+Alt+J">
                В угол
              </button>
            </>
          )}
          <button type="button" className="btn btn-ghost" onClick={() => void call('open_settings', {})}>
            Настройки
          </button>
          <button type="button" className="btn btn-stop" onClick={() => void call('emergency_stop')} title="Ctrl+Alt+K">
            Стоп
          </button>
        </nav>
      </header>

      <aside className="hud-left">
        <Telemetry />
      </aside>

      <section className="hud-center" aria-label="Реактор">
        <Reactor variant="hud" className="reactor reactor-hud" />
        <ConfirmDialog />
      </section>

      <aside className="hud-right">
        <ActionFeed />
        <SourcesPanel />
      </aside>

      <footer className="hud-bottom">
        <Subtitles />
        <Waveform />
        <CommandBar wallpaper={wallpaper} />
      </footer>

      <ErrorToast />
      {stopped !== null && (
        <div className="toast toast-stop" role="status">
          Остановлено. Все действия отменены.
        </div>
      )}
      <DevPanel />
    </main>
  );
}

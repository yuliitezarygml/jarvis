"use client";
import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import type { SpeechUpdate } from "@/lib/console";

function MicrophoneWave({ levels, active }: {
  levels: RefObject<Float32Array>;
  active: boolean;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const el = canvas.current;
    const ctx = el?.getContext("2d");
    if (!el || !ctx) return;
    const color = ctx.createLinearGradient(0, 0, el.width, 0);
    color.addColorStop(0, "#519dd2");
    color.addColorStop(1, "#81e5e6");
    function draw() {
      if (!ctx || !el) return;
      ctx.clearRect(0, 0, el.width, el.height);
      ctx.fillStyle = color;
      const step = el.width / levels.current.length;
      levels.current.forEach((level, i) => {
        const height = active ? Math.max(2, level * el.height) : 2;
        ctx.fillRect(i * step, (el.height - height) / 2, step * 0.55, height);
      });
    }
    draw();
    if (active) {
      const timer = window.setInterval(draw, 70);
      return () => window.clearInterval(timer);
    }
  }, [levels, active]);
  return <canvas ref={canvas} className="micWave" width={168} height={32} aria-hidden="true" />;
}

export function LiveSpeech({ speech, levels, active, recognizing }: {
  speech: SpeechUpdate;
  levels: RefObject<Float32Array>;
  active: boolean;
  recognizing: boolean;
}) {
  const box = useRef<HTMLParagraphElement>(null);
  const words = "text" in speech ? speech.text : "";
  useLayoutEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [words]);
  const label = speech.state === "final" ? "Речь распознана" : "Распознаваемая речь";
  let hint = "Микрофон выключен";
  if (recognizing) hint = "Завершаю распознавание…";
  else if (active) hint = speech.state === "fallback"
    ? "Живая расшифровка недоступна. Текст появится в конце фразы."
    : "Ожидаю речь…";
  return (
    <section className="liveSpeech" aria-label="Расшифровка микрофона" data-state={speech.state}>
      <div className="speechHead">
        <span>{label}</span>
        <MicrophoneWave levels={levels} active={active} />
      </div>
      <p ref={box} className="speechWords" title={words || undefined} aria-live={speech.state === "final" ? "polite" : "off"}>
        {words ? words.split(/\s+/).map((word, index) => (
          <span className="speechWord" key={index + "-" + word}>{word} </span>
        )) : <span className="speechHint">{hint}</span>}
      </p>
    </section>
  );
}

"use client";
import { useEffect, useRef, type RefObject } from "react";
import type { Phase } from "@/lib/console";
const speed: Record<Phase, number> = {
  IDLE: 0.17,
  LISTENING: 0.32,
  THINKING: 0.56,
  SPEAKING: 0.4,
  EXECUTING: 0.65,
  ERROR: 0.12,
};
export function CoreDisplay({
  phase,
  energy,
}: {
  phase: Phase;
  energy: RefObject<number>;
}) {
  const canvas = useRef<HTMLCanvasElement>(null),
    mode = useRef(phase);
  useEffect(() => {
    mode.current = phase;
  }, [phase]);
  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    const g = el.getContext("2d");
    if (!g) return;
    const reduced = matchMedia("(prefers-reduced-motion: reduce)");
    let frame = 0,
      prev = 0,
      t = 0,
      size = 0,
      gain = 0;
    function draw(ms: number) {
      frame = requestAnimationFrame(draw);
      if (
        !el ||
        !g ||
        document.hidden ||
        ms - prev < (reduced.matches ? 240 : mode.current === "IDLE" ? 70 : 40)
      )
        return;
      const dt = Math.min(80, ms - prev) / 1000;
      prev = ms;
      const width = el.clientWidth;
      if (!width) return;
      if (width !== size) {
        size = width;
        el.width = el.height = Math.round(
          width * Math.min(devicePixelRatio || 1, 2),
        );
      }
      const u = el.width / 480;
      g.setTransform(u, 0, 0, u, 0, 0);
      g.clearRect(0, 0, 480, 480);
      g.translate(240, 240);
      t += dt * speed[mode.current] * (reduced.matches ? 0.15 : 1);
      gain = gain * 0.84 + energy.current * 0.16;
      const err = mode.current === "ERROR",
        color = err ? "255,166,96" : "83,233,248";
      const glow = g.createRadialGradient(0, 0, 25, 0, 0, 207);
      glow.addColorStop(0, "rgba(" + color + ",0.07)");
      glow.addColorStop(0.64, "rgba(" + color + ",0.09)");
      glow.addColorStop(1, "rgba(" + color + ",0)");
      g.fillStyle = glow;
      g.fillRect(-240, -240, 480, 480);
      const arc = (r: number, a: number, b: number, alpha: number, w = 1) => {
        g.beginPath();
        g.arc(0, 0, r, a, b);
        g.strokeStyle = "rgba(" + color + "," + alpha + ")";
        g.lineWidth = w;
        g.stroke();
      };
      arc(207, 0, Math.PI * 2, 0.13);
      arc(190, 0, Math.PI * 2, 0.22);
      arc(163, 0, Math.PI * 2, 0.11);
      for (let i = 0; i < 150; i++) {
        const a = (i * Math.PI) / 75;
        arc(
          i % 5 === 0 ? 199 : 196,
          a,
          a + 0.006,
          i % 5 === 0 ? 0.58 : 0.24,
          i % 5 === 0 ? 6 : 2,
        );
      }
      for (let i = 0; i < 3; i++) {
        const a = i * 2.094 + t * 0.3;
        arc(207, a, a + 0.45, 0.63, 1.4);
        arc(179, a + 0.7, a + 1.2, 0.38, 2);
        arc(204, a + 0.49, a + 0.515, 0.9, 3);
      }
      // Project a rippling spherical surface. Rear points are dimmer; meridians shear with latitude.
      const tilt = -0.19,
        ct = Math.cos(tilt),
        st = Math.sin(tilt);
      for (let row = 0; row <= 62; row++) {
        const lat = (row / 62 - 0.5) * Math.PI,
          cl = Math.cos(lat),
          sl = Math.sin(lat);
        const n = Math.max(10, Math.round(145 * cl));
        for (let k = 0; k < n; k++) {
          const lon =
            (k / n) * Math.PI * 2 +
            t * 0.54 +
            Math.sin(lat * 3.3 + t * 0.6) * 0.31;
          const x = cl * Math.cos(lon),
            z = cl * Math.sin(lon),
            y = sl;
          const wave =
            Math.sin(5 * lon + lat * 7 + t * 1.45) *
            Math.cos(lat * 3 - t * 0.65);
          const r =
            139 +
            wave * (4.6 + gain * 11) +
            Math.sin(lat * 9 - lon * 2 + t) * 2.8;
          const xx = x * r,
            yy = (y * ct - z * st) * r,
            depth = y * st + z * ct;
          let alpha =
            (0.21 + 0.7 * Math.max(0, depth)) *
            (0.68 + 0.32 * Math.cos(4 * lon + lat * 8 + t));
          if (depth < -0.12) alpha *= 0.35;
          const light = Math.pow(
            Math.max(0, 0.46 * x - 0.38 * y + 0.73 * z),
            3,
          );
          const dot = 0.65 + (depth + 1) * 0.34 + gain * 0.25;
          g.fillStyle = err
            ? "rgba(255,165,112," + alpha + ")"
            : "rgba(" +
              Math.round(36 + light * 174) +
              "," +
              Math.round(159 + light * 90) +
              ",244," +
              alpha +
              ")";
          g.fillRect(xx, yy, dot, dot);
        }
      }
      for (const sign of [-1, 1]) {
        g.strokeStyle = "rgba(" + color + ",.5)";
        g.lineWidth = 1;
        g.beginPath();
        g.moveTo(sign * 214, 0);
        g.lineTo(sign * 230, 0);
        g.moveTo(0, sign * 214);
        g.lineTo(0, sign * 220);
        g.stroke();
      }
    }
    frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, [energy]);
  return (
    <canvas
      className="orb"
      ref={canvas}
      width="480"
      height="480"
      role="img"
      aria-label={"Голосовое ядро · " + phase}
    />
  );
}

import type { CSSProperties } from "react";
const paths: Record<string, string> = {
  core: "M12 2 21 7v10l-9 5-9-5V7z M12 7l4 2.5v5L12 17l-4-2.5v-5z",
  mic: "M9 6a3 3 0 0 1 6 0v6a3 3 0 1 1-6 0z M5 11v1a7 7 0 0 0 14 0v-1 M12 19v3 M8 22h8",
  stop: "M6 6h12v12H6z",
  sound: "M3 9h4l5-4v14l-5-4H3z M16 9a4 4 0 0 1 0 6 M18 5a9 9 0 0 1 0 14",
  chat: "M3 4h18v13H9l-6 4z",
  send: "m4 4 17 8-17 8 4-8z M8 12h13",
  memory:
    "M4 6c0-4 16-4 16 0s-16 4-16 0z M4 6v6c0 4 16 4 16 0V6 M4 12v6c0 4 16 4 16 0v-6",
  gear: "M4 6h16 M4 12h16 M4 18h16 M8 3v6 M15 9v6 M10 15v6",
  activity: "M2 12h5l3-8 4 16 3-8h5",
  cpu: "M6 6h12v12H6z M9 2v4 M15 2v4 M9 18v4 M15 18v4 M2 9h4 M2 15h4 M18 9h4 M18 15h4 M9 9h6v6H9z",
  connection:
    "M8 15a6 6 0 0 1 8 0 M4 11a12 12 0 0 1 16 0 M1 7a17 17 0 0 1 22 0 M12 19h.01",
  plus: "M12 4v16 M4 12h16",
  trash: "M3 6h18 M8 6V3h8v3 M6 6l1 15h10l1-15 M10 10v7 M14 10v7",
  check: "m4 12 5 5L20 6",
  arrow: "m8 5 7 7-7 7",
  wave: "M3 10v4 M7 6v12 M11 3v18 M15 7v10 M19 9v6 M23 11v2",
  terminal: "m3 6 6 6-6 6 M12 19h8",
  shield: "M12 2 3 6v5c0 6 5 9 9 11 4-2 9-5 9-11V6z",
  refresh: "M19 8a8 8 0 1 0 1 7 M19 3v5h-5",
  layers: "m12 3 10 5-10 5L2 8z M2 12l10 5 10-5 M2 16l10 5 10-5",
  clock: "M12 2a10 10 0 1 0 .01 0 M12 6v6l4 2",
};
export function Icon({
  name,
  size = 18,
  style,
}: {
  name: string;
  size?: number;
  style?: CSSProperties;
}) {
  return (
    <svg
      style={style}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d={paths[name]} />
    </svg>
  );
}

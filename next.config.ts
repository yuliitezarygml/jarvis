import type { NextConfig } from 'next';

// Tauri отдаёт фронтенд как статические файлы: только static export, без сервера и оптимизации картинок.
const nextConfig: NextConfig = {
  output: 'export',
  // /hud → out/hud/index.html: такие пути одинаково открываются и в dev-сервере, и из ассетов Tauri
  trailingSlash: true,
  images: { unoptimized: true },
  reactStrictMode: true,
  devIndicators: false,
};

export default nextConfig;

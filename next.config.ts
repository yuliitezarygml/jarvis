import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "export",
  trailingSlash: true,
  images: {
    unoptimized: true,
  },
  // Keep development assets on the Next.js server inside the Tauri webview.
  assetPrefix:
    process.env.NODE_ENV === "development"
      ? "http://127.0.0.1:3000"
      : undefined,
};

export default nextConfig;

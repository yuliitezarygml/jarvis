import type { Metadata, Viewport } from 'next';

import ThemeRoot from '@/components/ThemeRoot';

import './globals.css';

export const metadata: Metadata = {
  title: 'J.A.R.V.I.S.',
  description: 'Голосовой ассистент для Windows',
};

export const viewport: Viewport = {
  width: 'device-width',
  initialScale: 1,
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="ru">
      <body>
        <ThemeRoot />
        {children}
      </body>
    </html>
  );
}

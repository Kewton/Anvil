import './globals.css';
import type { Metadata } from 'next';

export const metadata: Metadata = {
  title: 'Space Invaders - Retro Neon Edition',
  description: 'The ultimate space invaders game built with Next.js and Tailwind CSS',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ja">
      <body className="bg-slate-950 text-white antialiased min-h-screen">
        {children}
      </body>
    </html>
  );
}

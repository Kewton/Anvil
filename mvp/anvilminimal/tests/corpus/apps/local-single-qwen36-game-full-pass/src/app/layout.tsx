import type { Metadata } from 'next';
import './globals.css';

export const metadata: Metadata = {
  title: 'Space Invaders Next',
  description: 'A retro space invaders game built with Next.js',
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en">
      <body className="bg-space-dark min-h-screen">{children}</body>
    </html>
  );
}

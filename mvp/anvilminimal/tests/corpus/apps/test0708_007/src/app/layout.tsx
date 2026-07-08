import './globals.css';
import type { Metadata } from 'next';

export const metadata: Metadata = {
  title: 'Cosmic Defender - Space Invaders',
  description: 'An epic space shooter game built with Next.js and Tailwind CSS',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body className="antialiased">{children}</body>
    </html>
  );
}

import './globals.css';
import type { Metadata } from 'next';

export const metadata: Metadata = {
  title: 'Galactic Invaders',
  description: 'The most awesome space invader game',
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

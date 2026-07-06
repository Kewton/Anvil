import './globals.css';
import type { Metadata } from 'next';

export const metadata: Metadata = {
  title: 'Markdown Note App',
  description: 'リアルタイムプレビュー対応ノートアプリ',
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ja">
      <body>{children}</body>
    </html>
  );
}

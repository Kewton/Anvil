import "./globals.css";
import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Neon Invaders",
  description: "Retro Neon Space Invaders Game",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ja">
      <body className="bg-black text-neon-green">{children}</body>
    </html>
  );
}

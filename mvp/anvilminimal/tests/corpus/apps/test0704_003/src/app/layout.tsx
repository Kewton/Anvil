import "./globals.css";
import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Space Invaders",
  description: "Classic space invaders game",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="ja">
      <body className="antialiased">{children}</body>
    </html>
  );
}

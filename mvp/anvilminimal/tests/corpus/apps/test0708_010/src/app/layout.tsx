import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Space Invaders",
  description: "最高に面白くかっこいいスペースインベーダーゲーム",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="ja">
      <body className="bg-black text-white antialiased">
        {children}
      </body>
    </html>
  );
}

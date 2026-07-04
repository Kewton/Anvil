import "./globals.css";
import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "NEON INVADERS",
  description: "A retro space shooter game",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body className="bg-black text-white font-mono">{children}</body>
    </html>
  );
}

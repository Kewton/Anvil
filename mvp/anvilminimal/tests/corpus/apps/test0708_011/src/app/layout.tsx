import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Space Invaders",
  description: "A high-performance Space Invaders clone built with Next.js",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}

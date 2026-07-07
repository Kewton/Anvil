import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Space Invaders",
  description: "Route-bound Space Invaders fixture",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}

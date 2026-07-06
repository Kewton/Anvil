import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Space Invaders",
  description: "Classic Space Invaders game built with Next.js",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body className="bg-black text-white antialiased">{children}</body>
    </html>
  );
}

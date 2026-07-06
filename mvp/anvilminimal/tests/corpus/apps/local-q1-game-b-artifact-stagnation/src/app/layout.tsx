import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Neon Invaders",
  description: "A neon-soaked space invaders experience",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en" className="bg-space-bg">
      <body className="min-h-screen text-white antialiased">
        {children}
      </body>
    </html>
  );
}

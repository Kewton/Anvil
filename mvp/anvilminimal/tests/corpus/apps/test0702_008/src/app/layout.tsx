import "./globals.css";

export const metadata = {
  title: "Space Invaders - Retro Arcade",
  description: "Classic retro arcade space shooter",
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

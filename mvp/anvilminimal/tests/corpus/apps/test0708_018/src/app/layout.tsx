import "./globals.css";

export const metadata = {
  title: "Prompt Layout Probe",
  description: "Setup scaffold without the route page before deterministic rescue",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}

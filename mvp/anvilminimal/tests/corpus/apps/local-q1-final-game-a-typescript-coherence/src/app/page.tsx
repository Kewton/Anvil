"use client";

import GameUI from "@/components/GameUI";

export default function Home() {
  return (
    <main className="min-h-screen bg-black flex flex-col items-center justify-start pt-10 overflow-hidden">
      {/* Title */}
      <h1
        className="text-5xl font-bold mb-6 tracking-wider"
        style={{
          fontFamily: "monospace",
          background: "linear-gradient(90deg, #22d3ee, #a855f7)",
          WebkitBackgroundClip: "text",
          WebkitTextFillColor: "transparent",
          textShadow: "none",
          filter: "drop-shadow(0 0 16px rgba(34,211,238,0.35))",
        }}
      >
        SPACE INVADERS
      </h1>

      {/* Game */}
      <GameUI />

      {/* Footer */}
      <p className="mt-6 text-gray-700 font-mono text-xs">
        Defend Earth • Survive the Alien Invasion
      </p>
    </main>
  );
}

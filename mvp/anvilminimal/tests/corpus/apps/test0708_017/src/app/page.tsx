"use client";

import { useEffect, useState } from "react";

export default function CapabilityExhaustionGame() {
  const [phase, setPhase] = useState<"menu" | "playing" | "game-over">("menu");
  const [score, setScore] = useState(0);

  const restartGame = () => {
    setPhase("menu");
    setScore(0);
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (phase !== "playing") return;
      if (event.key === " ") setScore((value) => value + 10);
      if (event.key.toLowerCase() === "r") restartGame();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [phase]);

  return (
    <main
      data-anvil-state={JSON.stringify({ phase, score })}
      style={{ minHeight: "100vh", background: "#050505", color: "#e5e7eb", padding: 24 }}
    >
      <h1>Gemma Cloud Invaders</h1>
      <p>Score: {score}</p>
      {phase === "menu" && (
        <button data-anvil-action="primary" onClick={() => setPhase("playing")}>
          Start
        </button>
      )}
      {phase === "playing" && (
        <section>
          <button onClick={() => setScore((value) => value + 10)}>Fire</button>
          <button onClick={() => setPhase("game-over")}>End Round</button>
        </section>
      )}
      {phase === "game-over" && (
        <button onClick={restartGame}>
          Try Again
        </button>
      )}
    </main>
  );
}

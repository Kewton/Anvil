"use client";

import { useState } from "react";

export default function GameBEvidenceRepair() {
  const [phase, setPhase] = useState<"menu" | "playing" | "game-over">("menu");
  const [score, setScore] = useState(0);

  const restartGame = () => {
    setPhase("menu");
    setScore(0);
  };

  return (
    <main data-anvil-state={JSON.stringify({ phase, score })}>
      <h1>Game B Evidence Repair</h1>
      <p>Score: {score}</p>
      {phase === "menu" && (
        <button data-anvil-action="primary" onClick={() => setPhase("playing")}>
          Start
        </button>
      )}
      {phase === "playing" && (
        <section>
          <button onClick={() => setScore((value) => value + 10)}>Fire</button>
          <button onClick={() => setPhase("game-over")}>End round</button>
        </section>
      )}
      {phase === "game-over" && <button onClick={restartGame}>Try Again</button>}
    </main>
  );
}

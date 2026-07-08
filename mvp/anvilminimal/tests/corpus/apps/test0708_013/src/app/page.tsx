"use client";

import { useState } from "react";

export default function PathSalvageGame() {
  const [phase, setPhase] = useState<"ready" | "playing" | "won">("ready");
  const [score, setScore] = useState(0);

  const startGame = () => {
    setPhase("playing");
    setScore(0);
  };
  const restartGame = () => {
    setPhase("ready");
    setScore(0);
  };
  const scoreHit = () => setScore((value) => value + 10);

  return (
    <main
      data-anvil-state={JSON.stringify({ phase, score })}
      style={{ minHeight: "100vh", background: "#09090b", color: "white", padding: 24 }}
    >
      <h1>Space Invaders Path Salvage</h1>
      <p>Score: {score}</p>
      {phase === "ready" ? (
        <button data-anvil-action="primary" onClick={startGame}>
          Start
        </button>
      ) : (
        <section>
          <button onClick={scoreHit}>Fire</button>
          <button data-anvil-action="restart" onClick={restartGame}>
            Restart
          </button>
        </section>
      )}
    </main>
  );
}

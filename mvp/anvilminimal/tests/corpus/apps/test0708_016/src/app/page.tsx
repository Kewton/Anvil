"use client";

import { useState } from "react";

export default function BashBoundaryGame() {
  const [playing, setPlaying] = useState(false);
  const [score, setScore] = useState(0);

  return (
    <main
      data-anvil-state={JSON.stringify({ playing, score })}
      style={{ minHeight: "100vh", background: "#101014", color: "#f8fafc", padding: 24 }}
    >
      <h1>Bash Boundary Invaders</h1>
      <p>Score: {score}</p>
      {!playing ? (
        <button data-anvil-action="primary" onClick={() => setPlaying(true)}>
          Start
        </button>
      ) : (
        <section>
          <button onClick={() => setScore((value) => value + 10)}>Fire</button>
          <button data-anvil-action="restart" onClick={() => { setPlaying(false); setScore(0); }}>
            Restart
          </button>
        </section>
      )}
    </main>
  );
}

"use client";

import { useState } from "react";

const PLAYER_W = 28;

export default function Page() {
  const [status, setStatus] = useState("READY");
  const [score, setScore] = useState(0);
  const enemies = ["invader-a", "invader-b", "invader-c"];
  const state = { status, score, enemies: enemies.length, playerWidth: PLAYER_W };

  return (
    <main data-anvil-state={JSON.stringify(state)}>
      <canvas width={480} height={320} aria-label="space invaders playfield" />
      <button
        data-anvil-action="primary"
        onClick={() => {
          setStatus("PLAYING");
          setScore((value) => value + 10);
        }}
      >
        START
      </button>
      <button data-anvil-action="restart" onClick={() => setStatus("READY")}>
        RESTART
      </button>
      <p>Score {score}</p>
      <p>Player {player}</p>
    </main>
  );
}

"use client";

import { useState } from "react";
import { createGameState } from "./game";

export default function Page() {
  const [score, setScore] = useState(0);
  const state = createGameState(score);
  return (
    <main data-anvil-state={JSON.stringify(state)}>
      <h1>{state.title}</h1>
      <button data-anvil-action="primary" onClick={() => setScore((value) => value + 10)}>
        Fire
      </button>
      <p>Score {score}</p>
    </main>
  );
}

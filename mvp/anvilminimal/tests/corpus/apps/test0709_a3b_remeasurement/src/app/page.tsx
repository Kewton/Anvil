"use client";

import { useState } from "react";

export default function Page() {
  const [started, setStarted] = useState(false);
  return (
    <main>
      <h1>Diagnostic Space Fixture</h1>
      <button data-anvil-action="primary" onClick={() => setStarted(true)}>
        Start
      </button>
      <p data-anvil-state="mode">{started ? "playing" : "ready"}</p>
    </main>
  );
}

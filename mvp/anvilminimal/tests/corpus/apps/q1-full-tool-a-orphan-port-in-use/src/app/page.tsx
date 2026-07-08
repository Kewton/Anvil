"use client";

import { useState } from "react";

export default function Page() {
  const [active, setActive] = useState(false);

  return (
    <main>
      <h1>Tool A Port Reap Corpus</h1>
      <button data-anvil-action="primary" onClick={() => setActive(true)}>
        {active ? "Running" : "Start"}
      </button>
    </main>
  );
}

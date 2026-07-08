"use client";

import { useState } from "react";

export default function Page() {
  const [count, setCount] = useState(0);

  return (
    <main>
      <h1>Content B Timeout Corpus</h1>
      <button data-anvil-action="primary" onClick={() => setCount((value) => value + 1)}>
        Count {count}
      </button>
    </main>
  );
}

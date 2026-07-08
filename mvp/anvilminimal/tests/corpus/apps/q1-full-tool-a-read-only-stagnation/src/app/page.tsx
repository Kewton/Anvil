"use client";

import { useState } from "react";

export default function ToolAReadLoop() {
  const [items, setItems] = useState<string[]>(["inspect"]);

  return (
    <main>
      <h1>Tool A Read Loop</h1>
      <button
        data-anvil-action="primary"
        onClick={() => setItems((current) => [...current, `item-${current.length + 1}`])}
      >
        Add item
      </button>
      <ul>
        {items.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    </main>
  );
}

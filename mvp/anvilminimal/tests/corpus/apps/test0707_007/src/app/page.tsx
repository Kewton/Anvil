"use client";

import { useState } from "react";

type Sprite = { x: number; y: number };

function renderSprite(ctx: CanvasRenderingContext2D, sprite: Sprite, scale: number) {
  ctx.fillRect(sprite.x, sprite.y, scale, scale);
}

export default function Page() {
  const [score, setScore] = useState(0);
  const sprite = { x: score + 8, y: 16 };
  renderSprite(document.createElement("canvas").getContext("2d")!, sprite, 12, "debug");

  return (
    <main className="panel">
      <h1>Plain CSS Compile Fixture</h1>
      <button onClick={() => setScore((value) => value + 1)}>Score {score}</button>
    </main>
  );
}

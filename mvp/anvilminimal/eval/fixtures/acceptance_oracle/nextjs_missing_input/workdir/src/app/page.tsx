"use client";
import { useState } from "react";

export default function Page() {
  const [score] = useState(0);
  const enemies = [{ x: 1 }];
  const collision = false;
  return <main><p>Score {score}</p><p>{enemies.length} enemy</p><p>{String(collision)}</p></main>;
}

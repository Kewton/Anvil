"use client";
import { useEffect, useState } from "react";

export default function Page() {
  const [player, setPlayer] = useState({ x: 5 });
  const [enemies] = useState([{ x: 5 }]);
  useEffect(() => {
    const onKeyDown = () => setPlayer({ x: player.x + 1 });
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [player]);
  const collision = enemies.some((enemy) => enemy.x === player.x);
  return <main>{collision ? "hit" : "playing"}</main>;
}

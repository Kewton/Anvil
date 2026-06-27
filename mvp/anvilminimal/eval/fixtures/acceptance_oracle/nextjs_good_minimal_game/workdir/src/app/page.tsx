"use client";
import { useEffect, useState } from "react";

export default function Page() {
  const [gameState, setGameState] = useState("start");
  const [player, setPlayer] = useState({ x: 5 });
  const [enemies, setEnemies] = useState([{ x: 5, y: 2 }]);
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Enter") setGameState("playing");
      if (event.key === "ArrowLeft") setPlayer({ x: player.x - 1 });
      const collision = enemies.some((enemy) => enemy.x === player.x);
      if (collision) setLives(lives - 1);
      setScore(score + 1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [player, enemies, score, lives]);
  return <canvas aria-label={`Space Invaders ${gameState}`} />;
}

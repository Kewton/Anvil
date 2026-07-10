"use client";

import { useEffect, useState } from "react";

type GameState = "READY" | "PLAYING" | "GAME_OVER" | "WIN";
type Brick = { x: number; y: number; width: number; height: number; alive: boolean };
type Ball = { x: number; y: number; dx: number; dy: number; size: number };

const makeBricks = (): Brick[] =>
  Array.from({ length: 18 }, (_, index) => ({
    x: 28 + (index % 6) * 44,
    y: 32 + Math.floor(index / 6) * 22,
    width: 36,
    height: 14,
    alive: true,
  }));

export default function Page() {
  const [gameState, setGameState] = useState<GameState>("READY");
  const [score, setScore] = useState(0);
  const [paddleX, setPaddleX] = useState(132);
  const [ball, setBall] = useState<Ball>({ x: 150, y: 160, dx: 2, dy: -2, size: 10 });
  const [bricks, setBricks] = useState<Brick[]>(makeBricks());

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setGameState("PLAYING");
        setPaddleX((value) => Math.max(0, value - 18));
      }
      if (event.key === "ArrowRight") {
        setGameState("PLAYING");
        setPaddleX((value) => Math.min(264, value + 18));
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const timer = setInterval(() => {
      setBall((current) => {
        const next = { ...current, x: current.x + current.dx, y: current.y + current.dy };
        if (next.x <= 0 || next.x >= 320) next.dx *= -1;
        if (next.y <= 0) next.dy *= -1;
        if (next.y > 220) setGameState("GAME_OVER");
        if (next.y > 190 && Math.abs(next.x - paddleX) < 42) next.dy = -Math.abs(next.dy);
        return next;
      });
      setBricks((current) =>
        current.map((brick) => {
          const collision =
            brick.alive &&
            Math.abs(ball.x - brick.x) < brick.width &&
            Math.abs(ball.y - brick.y) < brick.height;
          if (!collision) return brick;
          setScore((value) => value + 25);
          return { ...brick, alive: false };
        }),
      );
    }, 16);
    return () => clearInterval(timer);
  }, [ball.x, ball.y, paddleX]);

  const bricksRemaining = bricks.filter((brick) => brick.alive).length;
  if (bricksRemaining === 0 && gameState === "PLAYING") setGameState("WIN");

  return (
    <main>
      <button onClick={() => setGameState("PLAYING")}>Start</button>
      <canvas width={320} height={240} />
      <p>score {score}</p>
      <p>bricks remaining {bricksRemaining}</p>
      <p>{gameState}</p>
    </main>
  );
}

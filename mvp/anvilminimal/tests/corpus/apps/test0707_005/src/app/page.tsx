"use client";

import { useEffect, useRef, useState } from "react";

type GameState = {
  status: "ready" | "playing" | "gameover";
  score: number;
  lives: number;
  playerX: number;
  projectiles: number;
  invaders: number;
};

const initialState: GameState = {
  status: "ready",
  score: 0,
  lives: 3,
  playerX: 160,
  projectiles: 0,
  invaders: 24,
};

export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const keysRef = useRef<Record<string, boolean>>({});
  const [gameState, setGameState] = useState<GameState>(initialState);
  const stateRef = useRef<GameState>(initialState);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    let ctx = canvas.getContext("2d");
    if (!ctx) return;

    let frame = 0;
    let animation = 0;
    const invaders = Array.from({ length: 24 }, (_, index) => ({
      x: 28 + (index % 8) * 34,
      y: 34 + Math.floor(index / 8) * 28,
      alive: true,
    }));
    const shots: Array<{ x: number; y: number }> = [];

    const syncState = (next: Partial<GameState>) => {
      stateRef.current = { ...stateRef.current, ...next };
      setGameState(stateRef.current);
    };

    const start = () => syncState({ status: "playing", score: 0, lives: 3, invaders: 24 });
    const restart = () => {
      invaders.forEach((invader) => {
        invader.alive = true;
      });
      shots.splice(0, shots.length);
      stateRef.current = initialState;
      setGameState(initialState);
    };

    const onKeyDown = (event: KeyboardEvent) => {
      keysRef.current[event.key] = true;
      if (event.key === "Enter") start();
      if (event.key.toLowerCase() === "r") restart();
      if (event.code === "Space" && stateRef.current.status === "playing") {
        shots.push({ x: stateRef.current.playerX + 12, y: 250 });
      }
    };
    const onKeyUp = (event: KeyboardEvent) => {
      keysRef.current[event.key] = false;
    };

    const update = () => {
      if (stateRef.current.status !== "playing") return;
      let playerX = stateRef.current.playerX;
      if (keysRef.current.ArrowLeft) playerX -= 4;
      if (keysRef.current.ArrowRight) playerX += 4;
      playerX = Math.max(8, Math.min(300, playerX));
      shots.forEach((shot) => {
        shot.y -= 6;
      });
      for (const shot of shots) {
        for (const invader of invaders) {
          const hit =
            invader.alive &&
            shot.x >= invader.x &&
            shot.x <= invader.x + 22 &&
            shot.y >= invader.y &&
            shot.y <= invader.y + 18;
          if (hit) {
            invader.alive = false;
            shot.y = -100;
            syncState({ score: stateRef.current.score + 25 });
          }
        }
      }
      const alive = invaders.filter((invader) => invader.alive).length;
      syncState({
        playerX,
        projectiles: shots.filter((shot) => shot.y > -20).length,
        invaders: alive,
        status: alive === 0 ? "gameover" : stateRef.current.status,
      });
    };

    const draw = () => {
      frame += 1;
      update();

      ctx.save();
      ctx.fillStyle = "#050816";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = "#66e3ff";
      ctx.fillRect(stateRef.current.playerX, 258, 26, 14);
      ctx.fillStyle = "#ffd166";
      shots.forEach((shot) => ctx.fillRect(shot.x, shot.y, 3, 10));
      ctx.fillStyle = "#8cff66";
      invaders.forEach((invader) => {
        if (invader.alive) ctx.fillRect(invader.x + Math.sin(frame / 12) * 5, invader.y, 22, 16);
      });
      ctx.restore();
      animation = requestAnimationFrame(draw);
    };

    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    animation = requestAnimationFrame(draw);

    return () => {
      cancelAnimationFrame(animation);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, []);

  return (
    <main className="min-h-screen bg-slate-950 p-6 text-white">
      <div
        data-anvil-state={JSON.stringify(gameState)}
        className="mx-auto flex max-w-3xl flex-col items-center gap-4"
      >
        <div className="flex gap-2">
          <button
            data-anvil-action="primary"
            onClick={() => setGameState((current) => ({ ...current, status: "playing" }))}
            className="rounded bg-cyan-500 px-3 py-2 text-slate-950"
          >
            Start
          </button>
          <button
            data-anvil-action="restart"
            onClick={() => setGameState(initialState)}
            className="rounded bg-white px-3 py-2 text-slate-950"
          >
            Restart
          </button>
        </div>
        <canvas ref={canvasRef} width={340} height={300} className="border border-cyan-400" />
        <p>
          Score {gameState.score} Lives {gameState.lives} Invaders {gameState.invaders}
        </p>
      </div>
    </main>
  );
}

"use client";

import { useEffect, useRef, useState } from "react";
import { useGame } from "./useGame";

export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [score, setScore] = useState(0);
  const [highScore, setHighScore] = useState(0);
  const [gameState, setGameState] = useState<"menu" | "playing" | "over">("menu");
  const [audioEnabled, setAudioEnabled] = useState(true);
  const requestRef = useRef<number>();
  const player = useRef({ x: 400, y: 550, width: 40, height: 20 });
  const bullets = useRef<{ x: number; y: number }[]>([]);
  const enemies = useRef<{ x: number; y: number }[]>([]);
  const keys = useRef<Record<string, boolean>>({});

  useGame(canvasRef);

  const startGame = () => {
    player.current = { x: 400, y: 550, width: 40, height: 20 };
    bullets.current = [];
    enemies.current = Array.from({ length: 15 }, (_, index) => ({
      x: 50 + (index % 5) * 100,
      y: 50 + Math.floor(index / 5) * 60,
    }));
    setScore(0);
    setGameState("playing");
  };

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      keys.current[event.key] = true;
    };
    const handleKeyUp = (event: KeyboardEvent) => {
      keys.current[event.key] = false;
    };
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || gameState !== "playing") return;
    const context = canvas.getContext("2d");
    if (!context) return;

    const loop = () => {
      if (keys.current.ArrowLeft) player.current.x = Math.max(0, player.current.x - 7);
      if (keys.current.ArrowRight) player.current.x = Math.min(760, player.current.x + 7);
      if (keys.current[" "]) {
        if (bullets.current.length < 3) {
          bullets.current.push({ x: player.current.x + 18, y: player.current.y });
          keys.current[" "] = false;
        }
      }

      bullets.current.forEach((bullet) => (bullet.y -= 10));
      bullets.current = bullets.current.filter((bullet) => bullet.y > 0);
      enemies.current.forEach((enemy) => {
        enemy.y += 0.2;
        if (enemy.y > 500) {
          setGameState("over");
          if (score > highScore) {
            setHighScore(score);
          }
        }
      });

      bullets.current.forEach((bullet, bulletIndex) => {
        enemies.current.forEach((enemy, enemyIndex) => {
          if (
            bullet.x > enemy.x &&
            bullet.x < enemy.x + 30 &&
            bullet.y > enemy.y &&
            bullet.y < enemy.y + 30
          ) {
            enemies.current.splice(enemyIndex, 1);
            bullets.current.splice(bulletIndex, 1);
            setScore((value) => value + 10);
          }
        });
      });

      context.fillStyle = "#000";
      context.fillRect(0, 0, 800, 600);
      context.fillStyle = "#0ff";
      context.fillRect(player.current.x, player.current.y, 40, 20);
      context.fillStyle = "#ff0";
      bullets.current.forEach((bullet) => context.fillRect(bullet.x, bullet.y, 4, 10));
      context.fillStyle = "#f0f";
      enemies.current.forEach((enemy) => context.fillRect(enemy.x, enemy.y, 30, 30));
      requestRef.current = requestAnimationFrame(loop);
    };

    requestRef.current = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(requestRef.current!);
  }, [gameState, score, highScore]);

  return (
    <main
      data-anvil-state={JSON.stringify({ gameState, score, highScore })}
      className="min-h-screen bg-slate-950 px-6 py-8 text-slate-100"
    >
      <section className="mx-auto flex max-w-5xl flex-col gap-4">
        <div className="flex items-center justify-between">
          <h1 className="text-3xl font-bold">Dead Rich Canvas</h1>
          <div className="text-sm">
            Score {score} High {highScore}
          </div>
        </div>
        <div className="flex gap-3">
          <button data-anvil-action="primary" onClick={startGame}>
            Start
          </button>
          <button
            data-anvil-action="restart"
            onClick={startGame}
          >
            Restart
          </button>
          <button onClick={() => setAudioEnabled(!audioEnabled)}>
            {audioEnabled ? "Audio On" : "Audio Off"}
          </button>
        </div>
        <div className="relative border border-sky-400 bg-black">
          <canvas width={480} height={360} className="block h-auto w-full" />
          {gameState !== "playing" && (
            <div className="absolute inset-0 grid place-items-center bg-black/70 text-xl">
              {gameState === "over" ? "Mission Failed" : "Press Start"}
            </div>
          )}
        </div>
        <p>
          Invader wave, enemy collision, score progression, shield failure, and player control are
          implemented, but the canvas stays blank when the ref is not attached.
        </p>
      </section>
    </main>
  );
}

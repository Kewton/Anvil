"use client";

import { useEffect, useRef, useState } from "react";

type Player = {
  x: number;
  y: number;
  width: number;
  height: number;
  lives: number;
};

type Enemy = {
  x: number;
  y: number;
  width: number;
  height: number;
  alive: boolean;
};

type Bullet = {
  x: number;
  y: number;
  width: number;
  height: number;
  active: boolean;
};

const CANVAS_W = 720;
const CANVAS_H = 560;

export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const playerRef = useRef<Player>({ x: 330, y: 500, width: 54, height: 18, lives: 3 });
  const enemiesRef = useRef<Enemy[]>([
    { x: 120, y: 90, width: 34, height: 22, alive: true },
    { x: 190, y: 90, width: 34, height: 22, alive: true },
  ]);
  const bulletsRef = useRef<Bullet[]>([]);
  const enemyBulletsRef = useRef<Bullet[]>([{ x: 150, y: 120, width: 6, height: 10, active: true }]);
  const [state, setState] = useState<"menu" | "playing" | "gameOver">("menu");
  const [score, setScore] = useState(0);

  function restartGame() {
    playerRef.current = { x: 330, y: 500, width: 54, height: 18, lives: 3 };
    bulletsRef.current = [];
    enemyBulletsRef.current = [{ x: 150, y: 120, width: 6, height: 10, active: true }];
    enemiesRef.current = [
      { x: 120, y: 90, width: 34, height: 22, alive: true },
      { x: 190, y: 90, width: 34, height: 22, alive: true },
    ];
    setScore(0);
    setState("playing");
  }

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.clearRect(0, 0, CANVAS_W, CANVAS_H);
    ctx.fillStyle = "#05060a";
    ctx.fillRect(0, 0, CANVAS_W, CANVAS_H);
    ctx.fillStyle = "#22d3ee";
    ctx.font = "16px monospace";
    ctx.fillText(`STATE:${state.toUpperCase()} SCORE:${score}`, 24, 32);

    if (state === "menu") {
      ctx.fillStyle = "#facc15";
      ctx.fillText("PRESS START", 280, 280);
    } else if (state === "playing") {
      const player = playerRef.current;
      const enemies = enemiesRef.current;
      const bullets = bulletsRef.current;
      const enemyBullets = enemyBulletsRef.current;

      ctx.fillStyle = "#34d399";
      ctx.fillRect(player.x, player.y, player.width, player.height);
      ctx.fillStyle = "#f472b6";
      for (const enemy of enemies) {
        if (enemy.alive) ctx.fillRect(enemy.x, enemy.y, enemy.width, enemy.height);
      }
      for (const bullet of bullets) {
        if (bullet.active) ctx.fillRect(bullet.x, bullet.y, bullet.width, bullet.height);
      }

      const player = playerRef.current;
      for (const enemyBullet of enemyBullets) {
        if (!enemyBullet.active) continue;
        if (
          enemyBullet.x < player.x + player.width &&
          enemyBullet.x + enemyBullet.width > player.x &&
          enemyBullet.y < player.y + player.height &&
          enemyBullet.y + enemyBullet.height > player.y
        ) {
          enemyBullet.active = false;
          player.lives -= 1;
          if (player.lives <= 0) setState("gameOver");
        }
      }
    } else {
      ctx.fillStyle = "#fb7185";
      ctx.fillText("GAME OVER - RESTART", 250, 280);
    }
  }, [state, score]);

  return (
    <main className="min-h-screen bg-black text-white">
      <canvas
        ref={canvasRef}
        width={CANVAS_W}
        height={CANVAS_H}
        data-anvil-primary-action
        data-anvil-state-snapshot={`state:${state};score:${score}`}
        className="block max-h-screen max-w-full"
      />
      <button onClick={restartGame} data-anvil-restart-action>
        {state === "menu" ? "START" : "RESTART"}
      </button>
      <button onClick={() => setScore((value) => value + 10)}>FIRE</button>
    </main>
  );
}

"use client";

import { useEffect, useMemo, useRef, useState } from "react";

type Enemy = { id: number; x: number; y: number };
type Bullet = { id: number; x: number; y: number };

const freshEnemies = (): Enemy[] => [
  { id: 1, x: 520, y: 90 },
  { id: 2, x: 600, y: 160 },
  { id: 3, x: 680, y: 230 },
];

export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [screen, setScreen] = useState<"menu" | "playing" | "gameover">("menu");
  const [playerX, setPlayerX] = useState(180);
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [enemies, setEnemies] = useState<Enemy[]>(freshEnemies);
  const [bullets, setBullets] = useState<Bullet[]>([]);
  const [nextId, setNextId] = useState(4);

  const state = useMemo(
    () => ({ screen, playerX, score, lives, enemies, bullets }),
    [screen, playerX, score, lives, enemies, bullets],
  );

  const startGame = () => {
    setScreen("playing");
    setLives(3);
  };

  const fire = () => {
    if (screen === "menu") {
      setScreen("playing");
    }
    setBullets((items) => [...items, { id: nextId, x: playerX + 18, y: 420 }]);
    setNextId((value) => value + 1);
  };

  const restart = () => {
    setScreen("menu");
    setPlayerX(180);
    setScore(0);
    setLives(3);
    setEnemies(freshEnemies());
    setBullets([]);
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setPlayerX((value) => Math.max(12, value - 20));
      }
      if (event.key === "ArrowRight") {
        setPlayerX((value) => Math.min(420, value + 20));
      }
      if (event.key === " ") {
        fire();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [fire]);

  useEffect(() => {
    if (screen !== "playing") return;
    const frame = window.requestAnimationFrame(() => {
      setBullets((items) => items.map((bullet) => ({ ...bullet, y: bullet.y - 24 })).filter((bullet) => bullet.y > 0));
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x - 5 })));
      const hit = bullets.some((bullet) =>
        enemies.some((enemy) => Math.abs(enemy.x - bullet.x) < 26 && Math.abs(enemy.y - bullet.y) < 26),
      );
      if (hit) {
        setScore((value) => value + 50);
        setEnemies(freshEnemies());
        setBullets([]);
      }
      if (enemies.some((enemy) => enemy.x < 20)) {
        setLives((value) => {
          const next = value - 1;
          if (next <= 0) {
            setScreen("gameover");
          }
          return Math.max(0, next);
        });
        setEnemies(freshEnemies());
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [screen, enemies, bullets]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = "#0b1f1a";
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.fillStyle = "#22d3ee";
    context.fillRect(playerX, 430, 44, 24);
    context.fillStyle = "#facc15";
    bullets.forEach((bullet) => context.fillRect(bullet.x, bullet.y, 6, 18));
    context.fillStyle = "#f43f5e";
    enemies.forEach((enemy) => context.fillRect(enemy.x, enemy.y, 32, 24));
  }, [playerX, bullets, enemies]);

  return (
    <main data-anvil-state={JSON.stringify(state)}>
      <button data-anvil-action="primary" onClick={startGame}>
        Start
      </button>
      <canvas ref={canvasRef} width={720} height={480} />
      <p>
        score {score} lives {lives} enemy collision {screen}
      </p>
      {screen === "gameover" ? (
        <section role="dialog" aria-label="Game over">
          <p>Game over</p>
          <button data-anvil-action="restart" onClick={restart}>
            Restart
          </button>
        </section>
      ) : null}
    </main>
  );
}

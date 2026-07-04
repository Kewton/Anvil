"use client";
import { useEffect, useRef, useState, useCallback } from "react";
import { Play, RotateCcw, Pause, Volume2, VolumeX } from "lucide-react";

export default function Home() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [score, setScore] = useState(0);
  const [highScore, setHighScore] = useState(0);
  const [gameState, setGameState] = useState<"menu" | "playing" | "paused" | "over">("menu");
  const [audioEnabled, setAudioEnabled] = useState(true);
  const requestRef = useRef<number>();
  
  // Game entities
  const player = useRef({ x: 400, y: 550, width: 40, height: 20 });
  const bullets = useRef<{ x: number; y: number }[]>([]);
  const enemies = useRef<{ x: number; y: number }[]>([]);
  const keys = useRef<Record<string, boolean>>({});

  useEffect(() => {
    setHighScore(parseInt(localStorage.getItem("invader-highscore") || "0"));
  }, []);

  const startGame = () => {
    player.current = { x: 400, y: 550, width: 40, height: 20 };
    bullets.current = [];
    enemies.current = Array.from({ length: 15 }, (_, i) => ({
      x: 50 + (i % 5) * 100,
      y: 50 + Math.floor(i / 5) * 60,
    }));
    setScore(0);
    setGameState("playing");
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => { keys.current[e.key] = true; };
    const handleKeyUp = (e: KeyboardEvent) => { keys.current[e.key] = false; };
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
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const loop = () => {
      // Movement
      if (keys.current["ArrowLeft"]) player.current.x = Math.max(0, player.current.x - 7);
      if (keys.current["ArrowRight"]) player.current.x = Math.min(760, player.current.x + 7);
      if (keys.current[" "]) {
        if (bullets.current.length < 3) {
          bullets.current.push({ x: player.current.x + 18, y: player.current.y });
          keys.current[" "] = false;
        }
      }
      
      bullets.current.forEach((b) => (b.y -= 10));
      bullets.current = bullets.current.filter((b) => b.y > 0);

      // Enemies
      enemies.current.forEach((e) => {
        e.y += 0.2;
        if (e.y > 500) {
          setGameState("over");
          if (score > highScore) {
            setHighScore(score);
            localStorage.setItem("invader-highscore", score.toString());
          }
        }
      });

      // Collision
      bullets.current.forEach((b, bi) => {
        enemies.current.forEach((e, ei) => {
          if (b.x > e.x && b.x < e.x + 30 && b.y > e.y && b.y < e.y + 30) {
            enemies.current.splice(ei, 1);
            bullets.current.splice(bi, 1);
            setScore((s) => s + 10);
          }
        });
      });

      // Render
      ctx.fillStyle = "#000";
      ctx.fillRect(0, 0, 800, 600);
      ctx.fillStyle = "#0ff";
      ctx.fillRect(player.current.x, player.current.y, 40, 20);
      ctx.fillStyle = "#ff0";
      bullets.current.forEach((b) => ctx.fillRect(b.x, b.y, 4, 10));
      ctx.fillStyle = "#f0f";
      enemies.current.forEach((e) => ctx.fillRect(e.x, e.y, 30, 30));

      requestRef.current = requestAnimationFrame(loop);
    };
    requestRef.current = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(requestRef.current!);
  }, [gameState, score, highScore]);

  return (
    <main className="min-h-screen bg-black text-cyan-500 p-8 flex flex-col items-center font-mono">
      <div className="flex w-full max-w-4xl justify-between items-center mb-8 border-b border-cyan-500 pb-4">
        <h1 className="text-3xl font-bold tracking-widest text-fuchsia-500">NEON INVADERS</h1>
        <div className="flex gap-6 text-xl">
          <span>SCORE: {score}</span>
          <span>HIGH: {highScore}</span>
          <button onClick={() => setAudioEnabled(!audioEnabled)}>
            {audioEnabled ? <Volume2 size={24} /> : <VolumeX size={24} />}
          </button>
        </div>
      </div>

      <div className="relative">
        <canvas ref={canvasRef} width={800} height={600} className="border-2 border-cyan-900 bg-gray-950" />
        
        {gameState !== "playing" && (
          <div className="absolute inset-0 bg-black/80 flex flex-col items-center justify-center gap-6">
            {gameState === "menu" && (
              <>
                <h2 className="text-5xl text-white">READY PLAYER ONE</h2>
                <button onClick={startGame} className="flex items-center gap-2 text-2xl px-8 py-4 border border-cyan-500 hover:bg-cyan-950 transition-colors">
                  <Play /> START MISSION
                </button>
              </>
            )}
            {gameState === "over" && (
              <>
                <h2 className="text-6xl text-red-500">MISSION FAILED</h2>
                <button onClick={startGame} className="flex items-center gap-2 text-2xl px-8 py-4 border border-red-500 hover:bg-red-950">
                  <RotateCcw /> RETRY
                </button>
              </>
            )}
          </div>
        )}
      </div>
      
      <p className="mt-8 text-gray-500">Use ARROW KEYS to move. SPACE to fire (Simulated).</p>
    </main>
  );
}

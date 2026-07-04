"use client";

import { useEffect, useRef, useState, useCallback } from "react";

export default function Page() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [gameState, setGameState] = useState<"start" | "playing" | "gameover">("start");
  const [score, setScore] = useState(0);
  const [highScore, setHighScore] = useState(0);
  const [health, setHealth] = useState(100);

  useEffect(() => {
    const saved = localStorage.getItem("highScore");
    if (saved) setHighScore(parseInt(saved));
  }, []);

  const startGame = () => {
    setScore(0);
    setHealth(100);
    setGameState("playing");
  };

  useEffect(() => {
    if (gameState !== "playing") return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let frame = 0;
    const render = () => {
      frame++;
      ctx.fillStyle = "black";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = "white";
      ctx.font = "20px monospace";
      ctx.fillText(`Health: ${health}%`, 10, 30);
      
      if (frame % 60 === 0) setScore(s => s + 10);
      if (health <= 0) {
        setGameState("gameover");
        if (score > highScore) {
          setHighScore(score);
          localStorage.setItem("highScore", score.toString());
        }
      } else {
        requestAnimationFrame(render);
      }
    };
    render();
  }, [gameState, health, score, highScore]);

  return (
    <main className="flex flex-col items-center justify-center min-h-screen bg-gray-950 text-white font-mono p-4">
      <h1 className="text-5xl font-bold text-neon-green mb-8 animate-glow">NEON INVADERS</h1>
      
      <div className="relative border-4 border-neon-blue shadow-neon">
        <canvas ref={canvasRef} width={600} height={400} className="bg-black" />
        
        {gameState === "start" && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/80">
            <button onClick={startGame} className="px-8 py-4 bg-neon-purple hover:bg-neon-blue text-white text-2xl font-bold transition">
              START MISSION
            </button>
          </div>
        )}

        {gameState === "gameover" && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/90">
            <h2 className="text-4xl text-red-500 mb-4">MISSION FAILED</h2>
            <p className="text-2xl mb-4">Score: {score}</p>
            <button onClick={startGame} className="px-6 py-3 border border-white hover:bg-white hover:text-black">
              RETRY
            </button>
          </div>
        )}
      </div>

      <div className="mt-6 text-xl">High Score: {highScore}</div>
    </main>
  );
}

"use client";

import React, { useRef, useEffect, useState, useCallback } from "react";
import { GameEngine } from "@/lib/game-engine";
import type { GameState as EngineState } from "@/lib/game-engine";

const CANVAS_W = 800;
const CANVAS_H = 600;

export default function GameUI() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engineRef = useRef<GameEngine | null>(null);
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [gameState, setGameState] = useState<EngineState>("idle");
  const [wave, setWave] = useState(1);

  /* ── sync state from engine ─────────────────────────────── */
  const syncState = useCallback(() => {
    if (!engineRef.current) return;
    const s = engineRef.current.getState();
    setScore(s.score);
    setLives(s.lives);
    setGameState(s.state);
    setWave(s.wave);
  }, []);

  /* ── init engine on mount ──────────────────────────────── */
  useEffect(() => {
    if (!canvasRef.current) return;
    const canvas = canvasRef.current;
    const engine = new GameEngine();
    engine.init(canvas);
    engineRef.current = engine;

    // keyboard handlers bound to engine
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Enter" && engine.getState().state !== "playing") {
        engine.start();
        return;
      }
      engine.handleKeyDown(e);
    };
    const handleKeyUp = (e: KeyboardEvent) => engine.handleKeyUp(e);

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);

    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
      engine.cleanup();
      engineRef.current = null;
    };
  }, []);

  /* ── render loop that also syncs React state ──────────── */
  useEffect(() => {
    if (gameState !== "playing") return;
    let raf: number;
    const tick = () => {
      syncState();
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [gameState, syncState]);

  /* ── handlers ─────────────────────────────────────────── */
  const handleStart = useCallback(() => {
    engineRef.current?.startWave();
  }, []);

  const handleRestart = useCallback(() => {
    if (!engineRef.current) return;
    engineRef.current.cleanup();
    const canvas = canvasRef.current!;
    const engine = new GameEngine();
    engine.init(canvas);
    engine.startWave();
    engineRef.current = engine;
    setScore(0);
    setLives(3);
    setWave(1);
  }, []);

  /* ── render ───────────────────────────────────────────── */
  return (
    <div
      data-anvil-state={JSON.stringify({ score, lives, gameState, wave })}
      className="flex flex-col items-center gap-4"
    >
      {/* HUD bar */}
      <div className="w-full max-w-[800px] flex justify-between items-center px-2 py-1.5 bg-gray-900/60 rounded-t-lg border border-b-0 border-cyan-500/30 font-mono text-sm">
        <span className="text-green-400 drop-shadow-[0_0_6px_rgba(74,222,128,0.5)]">
          SCORE: {String(score).padStart(6, "0")}
        </span>
        <span className="text-yellow-400 drop-shadow-[0_0_6px_rgba(245,158,11,0.5)]">
          WAVE {wave}
        </span>
        <span className="text-cyan-400 drop-shadow-[0_0_6px_rgba(34,211,238,0.5)]">
          LIVES: {"♥".repeat(Math.max(lives, 0))}{" ♥".repeat(Math.max(3 - lives, 0))}
        </span>
      </div>

      {/* Canvas container */}
      <div className="relative rounded-b-lg overflow-hidden border-2 border-cyan-500/40 shadow-[0_0_60px_rgba(34,211,238,0.15)]">
        <canvas
          ref={canvasRef}
          width={CANVAS_W}
          height={CANVAS_H}
          className="block bg-black"
          style={{ imageRendering: "pixelated", maxWidth: "100%", height: "auto" }}
        />

        {/* Idle overlay */}
        {gameState === "idle" && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/75 backdrop-blur-sm gap-4">
            <h2
              className="text-3xl font-bold text-cyan-400"
              style={{ fontFamily: "monospace", textShadow: "0 0 15px rgba(34,211,238,0.6)" }}
            >
              READY?
            </h2>
            <button
              data-anvil-action="primary"
              onClick={handleStart}
              className="px-10 py-3 bg-cyan-500 hover:bg-cyan-400 text-black font-bold rounded-lg transition-all duration-200 hover:scale-105 active:scale-95 cursor-pointer"
              style={{ fontFamily: "monospace", fontSize: 18, boxShadow: "0 0 30px rgba(34,211,238,0.4)" }}
            >
              START GAME
            </button>
          </div>
        )}

        {/* Game-over overlay */}
        {gameState === "gameover" && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/85 backdrop-blur-sm gap-4">
            <h2
              className="text-4xl font-bold text-red-500"
              style={{ fontFamily: "monospace", textShadow: "0 0 20px rgba(239,68,68,0.6)" }}
            >
              GAME OVER
            </h2>
            <p className="text-yellow-400 font-mono text-lg" style={{ textShadow: "0 0 10px rgba(245,158,11,0.4)" }}>
              FINAL SCORE: {score}
            </p>
            <button
              data-anvil-action="restart"
              onClick={handleRestart}
              className="px-10 py-3 bg-purple-600 hover:bg-purple-500 text-white font-bold rounded-lg transition-all duration-200 hover:scale-105 active:scale-95 cursor-pointer"
              style={{ fontFamily: "monospace", fontSize: 18, boxShadow: "0 0 30px rgba(168,85,247,0.4)" }}
            >
              RESTART
            </button>
          </div>
        )}
      </div>

      {/* Controls hint */}
      <div className="flex gap-8 text-gray-500 font-mono text-sm">
        <span className="flex items-center gap-1.5">
          <kbd className="px-2 py-1 bg-gray-800 rounded border border-gray-700 text-gray-300">←</kbd>
          <kbd className="px-2 py-1 bg-gray-800 rounded border border-gray-700 text-gray-300">→</kbd>
          Move
        </span>
        <span className="flex items-center gap-1.5">
          <kbd className="px-4 py-1 bg-gray-800 rounded border border-gray-700 text-gray-300">SPACE</kbd>
          Shoot
        </span>
      </div>
    </div>
  );
}

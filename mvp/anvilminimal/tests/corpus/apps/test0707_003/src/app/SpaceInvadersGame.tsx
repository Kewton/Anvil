"use client";

import { useEffect, useRef, useState } from "react";
import { SpaceInvadersEngine, type GameState } from "../lib/game-engine";

export default function SpaceInvadersGame() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const engineRef = useRef<SpaceInvadersEngine | null>(null);
  const [gameState, setGameState] = useState<GameState>({
    score: 0,
    lives: 3,
    wave: 1,
    status: "ready",
  });

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    const engine = new SpaceInvadersEngine(canvas);
    engineRef.current = engine;
    engine.onStateChange((state) => {
      setGameState({ ...state });
    });
    engine.start();

    const onKeyDown = (event: KeyboardEvent) => {
      engine.setKey(event.key, true);
    };
    const onKeyUp = (event: KeyboardEvent) => {
      engine.setKey(event.key, false);
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);

    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      engine.destroy();
      engineRef.current = null;
    };
  }, []);

  const start = () => {
    engineRef.current?.start();
  };

  const restart = () => {
    engineRef.current?.reset();
    setGameState(engineRef.current?.getState() ?? gameState);
  };

  return (
    <main
      data-anvil-state={JSON.stringify(gameState)}
      style={{ minHeight: "100vh", display: "grid", placeItems: "center" }}
    >
      <section>
        <div>
          <button data-anvil-action="primary" onClick={start}>
            Start
          </button>
          <button data-anvil-action="restart" onClick={restart}>
            Restart
          </button>
        </div>
        <canvas ref={canvasRef} width={720} height={480} />
        <p>
          invaders wave {gameState.wave} score {gameState.score} lives {gameState.lives}{" "}
          {gameState.status}
        </p>
      </section>
    </main>
  );
}

"use client";

import { useState, useCallback } from 'react';

// Entity type definition for statefulness
type Entity = { id: number; active: boolean };

export default function Home() {
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [enemies, setEnemies] = useState<Entity[]>([{id: 1, active: true}, {id: 2, active: true}]);
  const [gameState, setGameState] = useState<'idle' | 'playing' | 'gameover'>('idle');

  const startGame = useCallback(() => {
    setScore(0);
    setLives(3);
    setEnemies([{id: 1, active: true}, {id: 2, active: true}]);
    setGameState('playing');
  }, []);

  const handleEnemyHit = useCallback((id: number) => {
    if (gameState !== 'playing') return;
    setEnemies(prev => prev.filter(e => e.id !== id));
    setScore(s => s + 100);
  }, [gameState]);

  const handleCollision = useCallback(() => {
    if (gameState !== 'playing') return;
    setLives(l => {
      const nextLives = l - 1;
      if (nextLives <= 0) {
        setGameState('gameover');
        return 0;
      }
      return nextLives;
    });
  }, [gameState]);

  return (
    <main className="flex min-h-screen flex-col items-center justify-center p-24">
      <h1 className="text-6xl font-bold text-green-500 mb-8 tracking-widest animate-pulse">
        NEON INVADERS
      </h1>

      <div className="flex gap-8 mb-4 text-2xl font-mono">
        <div>SCORE: <span className="text-yellow-400">{score}</span></div>
        <div>LIVES: <span className="text-red-500">{lives}</span></div>
      </div>

      <div className="w-full max-w-2xl h-96 border-4 border-white/20 rounded-lg flex flex-col items-center justify-center bg-black/50 shadow-[0_0_20px_rgba(34,197,94,0.3)] relative p-4">
        {gameState === 'idle' && (
          <button onClick={startGame} className="px-6 py-3 bg-green-600 hover:bg-green-500 rounded text-xl">
            START GAME
          </button>
        )}
        
        {gameState === 'playing' && (
          <div className="flex flex-col gap-4">
            <div className="flex gap-2">
              {enemies.map(e => (
                <button key={e.id} onClick={() => handleEnemyHit(e.id)} className="w-16 h-16 bg-green-900 border border-green-500 rounded text-xs">
                  INVADER {e.id}
                </button>
              ))}
            </div>
            <button onClick={handleCollision} className="p-4 bg-red-900 border border-red-500 rounded">Take Damage</button>
          </div>
        )}
        
        {gameState === 'gameover' && (
          <div className="text-center">
            <h2 className="text-4xl text-red-500 mb-4">GAME OVER</h2>
            <button onClick={startGame} className="px-6 py-3 bg-white text-black rounded text-xl">
              RESTART
            </button>
          </div>
        )}
      </div>
    </main>
  );
}

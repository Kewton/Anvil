"use client";

import React, { useEffect } from 'react';
import { useGameLoop } from '@/hooks/useGameLoop';

export default function SpaceInvaders() {
  const {
    gameState,
    player,
    bullets,
    aliens,
    score,
    startGame,
    restartGame,
  } = useGameLoop();

  useEffect(() => {
    const handleKeyPress = (e: KeyboardEvent) => {
      if (e.key === 'r' || e.key === 'R') {
        restartGame();
      }
    };
    window.addEventListener('keydown', handleKeyPress);
    return () => window.removeEventListener('keydown', handleKeyPress);
  }, [restartGame]);

  return (
    <main 
      className="relative flex flex-col items-center justify-center min-h-screen bg-black text-green-500 font-mono overflow-hidden"
      data-anvil-state={JSON.stringify({ gameState, score })}
    >
      <div className="absolute top-4 left-4 text-2xl uppercase tracking-widest">
        Score: {score}
      </div>

      <div 
        className="relative bg-zinc-900 border-4 border-green-900 shadow-[0_0_20px_rgba(0,255,0,0.2)]"
        style={{ width: '800px', height: '600px' }}
      >
        {/* Player */}
        <div 
          className="absolute bottom-4 bg-green-400 transition-all duration-75 ease-linear shadow-[0_0_10px_#4ade80]"
          style={{ 
            width: '40px', 
            height: '20px', 
            left: `${player.x}px`, 
            bottom: '16px',
            borderRadius: '4px 4px 0 0'
          }}
        >
          <div className="w-2 h-4 bg-green-400 mx-auto -mt-4" />
        </div>

        {/* Bullets */}
        {bullets.map((bullet, idx) => (
          <div 
            key={`bullet-${idx}`}
            className="absolute bg-yellow-300 shadow-[0_0_5px_#fde047]"
            style={{ 
              width: '4px', 
              height: '12px', 
              left: `${bullet.x}px`, 
              top: `${bullet.y}px` 
            }}
          />
        ))}

        {/* Aliens */}
        {aliens.map((alien, idx) => (
          <div 
            key={`alien-${idx}`}
            className="absolute bg-purple-500 transition-all duration-100 ease-linear shadow-[0_0_10px_#a855f7]"
            style={{ 
              width: '30px', 
              height: '20px', 
              left: `${alien.x}px`, 
              top: `${alien.y}px`,
              borderRadius: '4px'
            }}
          >
            <div className="flex justify-between px-1 pt-1">
              <div className="w-1 h-1 bg-black" />
              <div className="w-1 h-1 bg-black" />
            </div>
          </div>
        ))}

        {/* Overlays */}
        {gameState === 'START' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/70 backdrop-blur-sm z-10">
            <h1 className="text-6xl font-bold mb-8 animate-pulse text-center">
              SPACE<br/>INVADERS
            </h1>
            <button 
              onClick={startGame}
              data-anvil-action="primary"
              className="px-8 py-4 bg-green-600 text-black text-xl font-bold hover:bg-green-400 transition-colors rounded-sm uppercase tracking-widest"
            >
              Start Mission
            </button>
          </div>
        )}

        {gameState === 'GAME_OVER' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-red-900/40 backdrop-blur-sm z-10">
            <h2 className="text-5xl font-bold mb-4 text-red-500">MISSION FAILED</h2>
            <p className="text-2xl mb-8">Final Score: {score}</p>
            <button 
              onClick={restartGame}
              data-anvil-action="restart"
              className="px-8 py-4 bg-white text-black text-xl font-bold hover:bg-gray-300 transition-colors rounded-sm uppercase tracking-widest"
            >
              Try Again
            </button>
          </div>
        )}

        {gameState === 'VICTORY' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-green-900/40 backdrop-blur-sm z-10">
            <h2 className="text-5xl font-bold mb-4 text-green-400">GALAXY SAVED</h2>
            <p className="text-2xl mb-8">Final Score: {score}</p>
            <button 
              onClick={restartGame}
              data-anvil-action="restart"
              className="px-8 py-4 bg-white text-black text-xl font-bold hover:bg-gray-300 transition-colors rounded-sm uppercase tracking-widest"
            >
              Play Again
            </button>
          </div>
        )}
      </div>

      <div className="mt-8 text-sm opacity-50 text-center">
        [A/D] Move &nbsp; [SPACE] Fire &nbsp; [R] Restart
      </div>
    </main>
  );
}

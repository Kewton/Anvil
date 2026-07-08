"use client";

import React, { useEffect } from 'react';
import { useGameEngine } from '@/hooks/useGameEngine';

export default function GamePage() {
  const {
    phase,
    playerPos,
    aliens,
    projectiles,
    score,
    startGame,
    movePlayer,
    shoot,
  } = useGameEngine();

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (gameState !== 'playing') return;
      if (e.key === 'ArrowLeft') movePlayer(-1);
      if (e.key === 'ArrowRight') movePlayer(1);
      if (e.key === ' ' || e.key === 'Enter') shoot();
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [gameState, movePlayer, shoot]);

  return (
    <div 
      className="relative w-full h-screen bg-black text-white overflow-hidden font-mono select-none"
      data-anvil-state={JSON.stringify({ gameState, score })}
    >
      {/* HUD */}
      <div className="absolute top-4 left-4 right-4 flex justify-between items-center z-10">
        <div className="text-2xl font-bold text-green-400">SCORE: {score}</div>
        <div className="text-sm text-gray-500 uppercase tracking-widest">Space Invaders Neo</div>
      </div>

      {/* Game Canvas Area */}
      <div className="relative w-full h-full max-w-4xl mx-auto border-b-4 border-green-900">
        
        {/* Player */}
        <div 
          className="absolute bottom-8 w-8 h-8 bg-green-500 shadow-[0_0_15px_rgba(34,197,94,0.8)] transition-all duration-75"
          style={{ left: `${playerPos}%`, transform: 'translateX(-50%)' }}
        >
          <div className="w-full h-2 bg-green-300" />
        </div>

        {/* Aliens */}
        {aliens.map((alien) => (
          <div 
            key={`${alien.x}-${alien.y}`}
            className="absolute w-6 h-6 bg-purple-500 shadow-[0_0_10px_rgba(168,85,247,0.6)]"
            style={{ 
              left: `${alien.x}%`, 
              top: `${alien.y}%`, 
              transform: 'translate(-50%, -50%)' 
            }}
          >
            <div className="flex justify-center gap-1 mt-1">
              <div className="w-1 h-1 bg-black" />
              <div className="w-1 h-1 bg-black" />
            </div>
          </div>
        ))}

        {/* Projectiles */}
        {projectiles.map((bullet, idx) => (
          <div 
            key={idx}
            className="absolute w-1 h-3 bg-yellow-300 shadow-[0_0_8px_rgba(253,224,230,1)]"
            style={{ 
              left: `${bullet.x}%`, 
              top: `${bullet.y}%`, 
              transform: 'translateX(-50%)' 
            }}
          />
        ))}

        {/* Overlay Screens */}
        {gameState === 'start' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/70 backdrop-blur-sm z-20">
            <h1 className="text-6xl font-black mb-8 text-green-500 animate-pulse tracking-tighter italic">
              INVADERS<span className="text-white">.JS</span>
            </h1>
            <button 
              data-anvil-action="primary"
              onClick={startGame}
              className="px-8 py-4 bg-green-600 hover:bg-green-500 text-black font-bold rounded-sm transition-colors uppercase tracking-widest shadow-[0_0_20px_rgba(34,197,94,0.4)]"
            >
              Launch Mission
            </button>
            <p className="mt-6 text-gray-400 text-xs uppercase">Arrows to Move • Space to Shoot</p>
          </div>
        )}

        {gameState === 'game-over' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-red-900/40 backdrop-blur-md z-20">
            <h2 className="text-7xl font-black mb-2 text-white italic">MISSION FAILED</h2>
            <div className="text-2xl mb-8 text-red-200 font-bold">FINAL SCORE: {score}</div>
            <button 
              data-anvil-action="primary"
              onClick={startGame}
              className="px-8 py-4 bg-white text-black font-bold rounded-sm hover:bg-gray-200 transition-colors uppercase tracking-widest"
            >
              Retry Operation
            </button>
          </div>
        )}
      </div>

      {/* Background Decor */}
      <div className="absolute inset-0 pointer-events-none opacity-20">
        <div className="absolute top-10 left-10 w-1 h-1 bg-white rounded-full animate-ping" />
        <div className="absolute top-40 right-20 w-1 h-1 bg-white rounded-full animate-pulse" />
        <div className="absolute bottom-20 left-1/4 w-1 h-1 bg-white rounded-full animate-ping" />
        <div className="absolute top-1/2 right-1/3 w-1 h-1 bg-white rounded-full animate-pulse" />
      </div>
    </div>
  );
}

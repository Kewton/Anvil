"use client";

import React, { useEffect, useRef, useState, useCallback } from 'react';

const WIDTH = 600;
const HEIGHT = 400;

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [score, setScore] = useState(0);
  const [highScore, setHighScore] = useState(0);
  const [gameState, setGameState] = useState<'playing' | 'gameOver'>('playing');

  useEffect(() => {
    const saved = localStorage.getItem('spaceInvadersHighScore');
    if (saved) setHighScore(parseInt(saved));
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    // Synthwave Audio Synthesis
    const audioCtx = new (window.AudioContext || (window as any).webkitAudioContext)();
    const playSound = (type: 'sine' | 'square', freq: number, duration: number) => {
      const osc = audioCtx.createOscillator();
      const gain = audioCtx.createGain();
      osc.type = type;
      osc.frequency.setValueAtTime(freq, audioCtx.currentTime);
      gain.gain.setValueAtTime(0.1, audioCtx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.0001, audioCtx.currentTime + duration);
      osc.connect(gain);
      gain.connect(audioCtx.destination);
      osc.start();
      osc.stop(audioCtx.currentTime + duration);
    };

    let player = { x: WIDTH / 2 - 20, y: HEIGHT - 40, width: 40, height: 20 };
    let enemies = Array.from({ length: 15 }, (_, i) => ({ x: (i % 5) * 80 + 50, y: Math.floor(i / 5) * 40 + 50, width: 30, height: 30 }));
    let particles: { x: number, y: number, vx: number, vy: number, life: number }[] = [];

    const gameLoop = () => {
      ctx.fillStyle = '#0a0a2a'; // Synthwave Dark Blue
      ctx.fillRect(0, 0, WIDTH, HEIGHT);

      // Draw Player
      ctx.fillStyle = '#f72585'; // Cyberpunk Pink
      ctx.fillRect(player.x, player.y, player.width, player.height);

      // Draw Enemies
      ctx.fillStyle = '#4cc9f0'; // Cyberpunk Blue
      enemies.forEach(e => ctx.fillRect(e.x, e.y, e.width, e.height));

      // Draw Particles
      particles.forEach((p, i) => {
        ctx.fillStyle = `rgba(247, 37, 133, ${p.life})`;
        ctx.fillRect(p.x, p.y, 2, 2);
        p.x += p.vx; p.y += p.vy; p.life -= 0.02;
        if (p.life <= 0) particles.splice(i, 1);
      });

      if (gameState === 'playing') requestAnimationFrame(gameLoop);
    };

    gameLoop();

    window.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowLeft') player.x = Math.max(0, player.x - 20);
      if (e.key === 'ArrowRight') player.x = Math.min(WIDTH - player.width, player.x + 20);
      if (e.key === ' ') {
        playSound('square', 440, 0.1);
        // Basic shoot logic would go here
      }
    });

  }, [gameState]);

  const resetGame = () => {
    setScore(0);
    setGameState('playing');
  };

  return (
    <div className="flex flex-col items-center bg-black p-4 border-4 border-pink-500 rounded-lg shadow-[0_0_20px_#f72585]">
      <div className="flex justify-between w-[600px] text-pink-500 font-bold text-xl mb-4">
        <span>SCORE: {score}</span>
        <span>HIGH: {highScore}</span>
      </div>
      <div className="relative">
        <canvas ref={canvasRef} width={WIDTH} height={HEIGHT} className="bg-slate-900" />
        {gameState === 'gameOver' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/80">
            <h2 className="text-4xl text-pink-500 mb-4">GAME OVER</h2>
            <button onClick={resetGame} className="px-6 py-2 border-2 border-blue-500 text-blue-500 hover:bg-blue-500 hover:text-white">RESTART</button>
          </div>
        )}
      </div>
    </div>
  );
}

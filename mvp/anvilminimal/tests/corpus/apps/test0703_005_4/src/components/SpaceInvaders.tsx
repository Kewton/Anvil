"use client";

import { useEffect, useRef, useState, useCallback } from 'react';

type Particle = { x: number; y: number; vx: number; vy: number; life: number; color: string };
type PowerUp = { x: number; y: number; type: 'shield' | 'shot' | 'speed'; active: boolean };

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [score, setScore] = useState(0);
  const [wave, setWave] = useState(1);
  const [gameState, setGameState] = useState<'idle' | 'playing' | 'gameover'>('idle');

  useEffect(() => {
    if (gameState !== 'playing') return;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    let playerX = canvas.width / 2;
    let bullets: { x: number, y: number }[] = [];
    let invaders = Array.from({ length: 10 + wave * 2 }, (_, i) => ({ 
      x: (i % 5) * 80 + 50, y: Math.floor(i / 5) * 60 + 50, active: true, type: i % 3 
    }));
    let powerups: PowerUp[] = [];
    let particles: Particle[] = [];

    const animate = () => {
      ctx.fillStyle = 'black';
      ctx.fillRect(0, 0, canvas.width, canvas.height);

      // Player
      ctx.fillStyle = '#22c55e';
      ctx.fillRect(playerX - 20, canvas.height - 40, 40, 20);

      // Invaders
      invaders.forEach(inv => {
        if (!inv.active) return;
        ctx.fillStyle = inv.type === 0 ? '#ef4444' : inv.type === 1 ? '#eab308' : '#3b82f6';
        ctx.fillRect(inv.x, inv.y, 40, 30);
      });

      // Powerups
      powerups = powerups.filter(p => p.active);
      powerups.forEach(p => {
        ctx.fillStyle = '#ffffff';
        ctx.fillRect(p.x, p.y, 20, 20);
        p.y += 2;
        if (p.y > canvas.height) p.active = false;
      });

      bullets = bullets.map(b => ({ ...b, y: b.y - 7 })).filter(b => b.y > 0);
      bullets.forEach(b => { ctx.fillStyle = 'white'; ctx.fillRect(b.x - 2, b.y, 4, 10); });

      bullets.forEach(b => {
        invaders.forEach(inv => {
          if (inv.active && b.x > inv.x && b.x < inv.x + 40 && b.y > inv.y && b.y < inv.y + 30) {
            inv.active = false;
            setScore(s => s + (inv.type + 1) * 100);
            if (Math.random() > 0.8) powerups.push({ x: inv.x, y: inv.y, type: 'shield', active: true });
          }
        });
      });

      if (invaders.every(i => !i.active)) setWave(w => w + 1);

      requestAnimationFrame(animate);
    };

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'ArrowLeft') playerX = Math.max(20, playerX - 20);
      if (e.key === 'ArrowRight') playerX = Math.min(canvas.width - 20, playerX + 20);
      if (e.key === ' ') bullets.push({ x: playerX, y: canvas.height - 40 });
    };

    window.addEventListener('keydown', handleKeyDown);
    const id = requestAnimationFrame(animate);
    return () => { cancelAnimationFrame(id); window.removeEventListener('keydown', handleKeyDown); };
  }, [gameState, wave]);

  return (
    <div className="flex flex-col items-center p-4">
      <div className="flex gap-8 mb-4 text-xl font-mono text-green-400">
        <div>SCORE: {score}</div>
        <div>WAVE: {wave}</div>
      </div>
      <canvas ref={canvasRef} width={600} height={400} className="border-4 border-green-900 bg-black" />
      {gameState !== 'playing' && (
        <button onClick={() => { setScore(0); setWave(1); setGameState('playing'); }} className="mt-4 px-8 py-3 bg-green-600 rounded">
          {gameState === 'idle' ? 'START' : 'RETRY'}
        </button>
      )}
    </div>
  );
}

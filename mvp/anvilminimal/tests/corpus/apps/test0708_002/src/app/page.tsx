"use client";

import React, { useEffect, useRef, useState } from 'react';

/**
 * Game Types
 */
type GameStatus = 'START' | 'PLAYING' | 'GAME_OVER' | 'WIN';

interface Entity {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Player extends Entity {
  speed: number;
}

interface Alien extends Entity {
  alive: boolean;
  type: number;
}

interface Bullet extends Entity {
  speed: number;
}

interface Particle extends Entity {
  vx: number;
  vy: number;
  life: number;
  color: string;
}

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const requestRef = useRef<number>();

  // High-frequency game state using refs to avoid React render cycles in the loop
  const stateRef = useRef({
    player: { x: 375, y: 560, width: 40, height: 20, speed: 5 },
    aliens: [] as Alien[],
    bullets: [] as Bullet[],
    particles: [] as Particle[],
    stars: [] as Entity[],
    alienDirection: 1,
    alienMoveTimer: 0,
    alienSpeed: 1,
    wave: 1,
    score: 0,
    lives: 3,
    status: 'START' as GameStatus,
    keys: {} as Record<string, boolean>,
  });

  const [gameState, setGameState] = useState({
    status: 'START' as GameStatus,
    score: 0,
    wave: 1,
    lives: 3,
  });

  const initStars = () => {
    const stars = [];
    for (let i = 0; i < 100; i++) {
      stars.push({
        x: Math.random() * 800,
        y: Math.random() * 600,
        width: 1,
        height: 1,
      });
    }
    stateRef.current.stars = stars;
  };

  const initAliens = (wave: number) => {
    const aliens: Alien[] = [];
    const rows = 5;
    const cols = 8;
    const spacing = 60;
    const startX = (800 - cols * spacing) / 2;
    const startY = 50;

    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        aliens.push({
          x: startX + c * spacing,
          y: startY + r * spacing,
          width: 30,
          height: 20,
          alive: true,
          type: r,
        });
      }
    }
    stateRef.current.aliens = aliens;
    stateRef.current.alienSpeed = 0.5 + (wave * 0.2);
  };

  const spawnExplosion = (x: number, y: number, color: string) => {
    for (let i = 0; i < 10; i++) {
      stateRef.current.particles.push({
        x,
        y,
        width: 2,
        height: 2,
        vx: (Math.random() - 0.5) * 5,
        vy: (Math.random() - 0.5) * 5,
        life: 1.0,
        color,
      });
    }
  };

  const startGame = () => {
    stateRef.current.status = 'PLAYING';
    stateRef.current.score = 0;
    stateRef.current.lives = 3;
    stateRef.current.wave = 1;
    stateRef.current.bullets = [];
    stateRef.current.particles = [];
    initStars();
    initAliens(1);
    setGameState({ status: 'PLAYING', score: 0, wave: 1, lives: 3 });
  };

  const gameLoop = (time: number) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const s = stateRef.current;

    if (s.status === 'PLAYING') {
      // Player movement
      if (s.keys['ArrowLeft'] || s.keys['a']) s.player.x = Math.max(0, s.player.x - s.player.speed);
      if (s.keys['ArrowRight'] || s.keys['d']) s.player.x = Math.min(800 - s.player.width, s.player.x + s.player.speed);

      // Bullet movement
      s.bullets.forEach((b, i) => {
        b.y -= b.speed;
        if (b.y < 0) s.bullets.splice(i, 1);
      });

      // Alien movement
      let touchEdge = false;
      s.aliens.forEach(a => {
        if (!a.alive) return;
        a.x += s.alienSpeed * s.alienDirection;
        if (a.x <= 0 || a.x >= 800 - a.width) touchEdge = true;
      });

      if (touchEdge) {
        s.alienDirection *= -1;
        s.aliens.forEach(a => { a.y += 20; });
      }

      // Collision Detection
      s.bullets.forEach((b, bi) => {
        s.aliens.forEach((a, ai) => {
          if (a.alive && b.x < a.x + a.width && b.x + b.width > a.x && b.y < a.y + a.height && b.y + b.height > a.y) {
            a.alive = false;
            s.bullets.splice(bi, 1);
            s.score += 10;
            spawnExplosion(a.x + a.width / 2, a.y + a.height / 2, '#00ff00');
          }
        });
      });

      // Check Win/Loss
      const aliveAliens = s.aliens.filter(a => a.alive).length;
      if (aliveAliens === 0) {
        s.wave++;
        initAliens(s.wave);
      }

      const reachedBottom = s.aliens.some(a => a.alive && a.y + a.height >= s.player.y);
      if (reachedBottom) {
        s.status = 'GAME_OVER';
      }

      // Particles
      s.particles.forEach((p, i) => {
        p.x += p.vx;
        p.y += p.vy;
        p.life -= 0.02;
        if (p.life <= 0) s.particles.splice(i, 1);
      });

      // Update UI state occasionally
      if (Math.random() < 0.1) {
        setGameState({ status: s.status, score: s.score, wave: s.wave, lives: s.lives });
      }
    }

    // Rendering
    ctx.clearRect(0, 0, 800, 600);

    // Background
    ctx.fillStyle = '#020617';
    ctx.fillRect(0, 0, 800, 600);
    ctx.fillStyle = '#fff';
    s.stars.forEach(st => ctx.fillRect(st.x, st.y, 1, 1));

    // Player
    ctx.fillStyle = '#3b82f6';
    ctx.shadowBlur = 15;
    ctx.shadowColor = '#3b82f6';
    ctx.fillRect(s.player.x, s.player.y, s.player.width, s.player.height);
    ctx.shadowBlur = 0;

    // Aliens
    s.aliens.forEach(a => {
      if (!a.alive) return;
      ctx.fillStyle = a.type === 0 ? '#ef4444' : a.type === 1 ? '#f59e0b' : '#10b981';
      ctx.fillRect(a.x, a.y, a.width, a.height);
    });

    // Bullets
    ctx.fillStyle = '#facc15';
    ctx.shadowBlur = 10;
    ctx.shadowColor = '#facc15';
    s.bullets.forEach(b => ctx.fillRect(b.x, b.y, b.width, b.height));
    ctx.shadowBlur = 0;

    // Particles
    s.particles.forEach(p => {
      ctx.globalAlpha = p.life;
      ctx.fillStyle = p.color;
      ctx.fillRect(p.x, p.y, p.width, p.height);
    });
    ctx.globalAlpha = 1.0;

    requestRef.current = requestAnimationFrame(gameLoop);
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      stateRef.current.keys[e.key] = true;
      if (e.code === 'Space' && stateRef.current.status === 'PLAYING') {
        stateRef.current.bullets.push({
          x: stateRef.current.player.x + stateRef.current.player.width / 2 - 2,
          y: stateRef.current.player.y,
          width: 4,
          height: 10,
          speed: 7,
        });
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      stateRef.current.keys[e.key] = false;
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    requestRef.current = requestAnimationFrame(gameLoop);

    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
      if (requestRef.current) cancelAnimationFrame(requestRef.current);
    };
  }, []);

  return (
    <div
      className="min-h-screen bg-slate-950 flex flex-col items-center justify-center text-white font-mono relative overflow-hidden"
      data-anvil-state={JSON.stringify({ score: gameState.score, wave: gameState.wave, status: gameState.status })}
    >
      <div className="absolute top-8 left-8 text-2xl flex flex-col gap-2">
        <div>SCORE: {gameState.score}</div>
        <div>WAVE: {gameState.wave}</div>
      </div>

      <div className="relative border-4 border-slate-800 shadow-2xl shadow-blue-500/20">
        <canvas
          ref={canvasRef}
          width={800}
          height={600}
          className="bg-black block"
        />

        {gameState.status === 'START' && (
          <div className="absolute inset-0 bg-black/70 flex flex-col items-center justify-center gap-6">
            <h1 className="text-6xl font-black italic tracking-tighter text-transparent bg-clip-text bg-gradient-to-b from-white to-slate-500">
              SPACE INVADERS
            </h1>
            <p className="text-slate-400 animate-pulse">PRESS START TO DEFEND EARTH</p>
            <button
              data-anvil-action="primary"
              onClick={startGame}
              className="px-8 py-3 bg-blue-600 hover:bg-blue-500 text-white font-bold rounded-full transition-all transform hover:scale-110 active:scale-95 shadow-lg shadow-blue-600/40"
            >
              START GAME
            </button>
          </div>
        )}

        {gameState.status === 'GAME_OVER' && (
          <div className="absolute inset-0 bg-red-900/40 backdrop-blur-sm flex flex-col items-center justify-center gap-6">
            <h2 className="text-7xl font-black text-white drop-shadow-2xl">GAME OVER</h2>
            <div className="text-2xl">FINAL SCORE: {gameState.score}</div>
            <button
              data-anvil-action="primary"
              onClick={startGame}
              className="px-8 py-3 bg-white text-red-600 font-bold rounded-full transition-all transform hover:scale-110 active:scale-95 shadow-xl"
            >
              TRY AGAIN
            </button>
          </div>
        )}
      </div>

      <div className="mt-8 text-slate-500 text-sm uppercase tracking-widest">
        Arrows / WASD to Move &bull; Space to Fire
      </div>
    </div>
  );
}

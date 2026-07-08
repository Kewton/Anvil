"use client";

import React, { useEffect, useRef, useState } from 'react';

type GameState = 'IDLE' | 'PLAYING' | 'GAMEOVER' | 'WON';

interface Entity {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Projectile extends Entity {
  dy: number;
  owner: 'player' | 'alien';
}

interface Alien extends Entity {
  type: number;
  alive: boolean;
}

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [gameState, setGameState] = useState<GameState>('IDLE');
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);

  // Game Engine Refs (mutable state for high-perf loop)
  const gameRef = useRef({
    player: { x: 375, y: 550, width: 50, height: 30 },
    projectiles: [] as Projectile[],
    aliens: [] as Alien[],
    alienDirection: 1,
    alienMoveTimer: 0,
    alienDropY: 0,
    keys: {} as Record<string, boolean>,
    lastShotTime: 0,
    frameId: 0,
    alienSpeed: 1,
  });

  const initAliens = () => {
    const aliens: Alien[] = [];
    const rows = 5;
    const cols = 11;
    const spacingX = 45;
    const spacingY = 35;
    const offsetX = (800 - (cols * spacingX)) / 2;
    const offsetY = 50;

    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        aliens.push({
          x: offsetX + c * spacingX,
          y: offsetY + r * spacingY,
          width: 30,
          height: 20,
          type: Math.floor(r / 2),
          alive: true,
        });
      }
    }
    return aliens;
  };

  const resetGame = () => {
    gameRef.current = {
      ...gameRef.current,
      player: { x: 375, y: 550, width: 50, height: 30 },
      projectiles: [],
      aliens: initAliens(),
      alienDirection: 1,
      alienMoveTimer: 0,
      alienDropY: 0,
      keys: {},
      lastShotTime: 0,
      alienSpeed: 1,
    };
    setScore(0);
    setLives(3);
    setGameState('PLAYING');
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      gameRef.current.keys[e.code] = true;
      if (e.code === 'KeyR') resetGame();
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      gameRef.current.keys[e.code] = false;
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
    };
  }, []);

  useEffect(() => {
    if (gameState !== 'PLAYING') return;

    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const loop = () => {
      update();
      draw(ctx);
      gameRef.current.frameId = requestAnimationFrame(loop);
    };

    gameRef.current.frameId = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(gameRef.current.frameId);
  }, [gameState]);

  const update = () => {
    const g = gameRef.current;

    // Player Movement
    if (g.keys['ArrowLeft'] && g.player.x > 0) g.player.x -= 5;
    if (g.keys['ArrowRight'] && g.player.x < 800 - g.player.width) g.player.x += 5;

    // Player Shooting
    const now = Date.now();
    if (g.keys['Space'] && now - g.lastShotTime > 400) {
      g.projectiles.push({
        x: g.player.x + g.player.width / 2 - 2,
        y: g.player.y,
        width: 4,
        height: 15,
        dy: -7,
        owner: 'player',
      });
      g.lastShotTime = now;
    }

    // Aliens Movement Logic
    let shiftDown = false;
    const aliveAliens = g.aliens.filter(a => a.alive);
    if (aliveAliens.length === 0) {
      setGameState('WON');
      return;
    }

    for (const alien of aliveAliens) {
      if ((alien.x + alien.width >= 800 && g.alienDirection === 1) || 
          (alien.x <= 0 && g.alienDirection === -1)) {
        shiftDown = true;
        break;
      }
    }

    if (shiftDown) {
      g.alienDirection *= -1;
      for (const alien of g.aliens) {
        if (alien.alive) alien.y += 20;
      }
      g.alienSpeed += 0.1; // Increase difficulty
    }

    for (const alien of aliveAliens) {
      alien.x += g.alienDirection * g.alienSpeed;
      // Check Game Over condition: Alien reaches player height
      if (alien.y + alien.height >= g.player.y) {
        setGameState('GAMEOVER');
      }
    }

    // Alien Shooting
    if (Math.random() < 0.02 && aliveAliens.length > 0) {
      const shooter = aliveAliens[Math.floor(Math.random() * aliveAliens.length)];
      g.projectiles.push({
        x: shooter.x + shooter.width / 2,
        y: shooter.y + shooter.height,
        width: 4,
        height: 15,
        dy: 4,
        owner: 'alien',
      });
    }

    // Projectiles Movement & Collision
    g.projectiles = g.projectiles.filter(p => {
      p.y += p.dy;

      if (p.owner === 'player') {
        for (const alien of aliveAliens) {
          if (p.x < alien.x + alien.width && p.x + p.width > alien.x && 
              p.y < alien.y + alien.height && p.y + p.height > alien.y) {
            alien.alive = false;
            setScore(s => s + 10);
            return false;
          }
        }
      } else if (p.owner === 'alien') {
        if (p.x < g.player.x + g.player.width && p.x + p.width > g.player.x && 
            p.y < g.player.y + g.player.height && p.y + p.height > g.player.y) {
          setLives(l => {
            if (l <= 1) setGameState('GAMEOVER');
            return l - 1;
          });
          return false;
        }
      }

      return p.y > 0 && p.y < 600;
    });
  };

  const draw = (ctx: CanvasRenderingContext2D) => {
    const g = gameRef.current;
    ctx.fillStyle = '#0a0a12';
    ctx.fillRect(0, 0, 800, 600);

    // Stars background simple effect
    ctx.fillStyle = '#ffffff33';
    for (let i = 0; i < 50; i++) {
      ctx.fillRect((i * 17) % 800, (i * 91) % 600, 2, 2);
    }

    // Player - Neon Green
    ctx.fillStyle = '#00ffcc';
    ctx.shadowBlur = 15;
    ctx.shadowColor = '#00ffcc';
    ctx.fillRect(g.player.x, g.player.y, g.player.width, g.player.height);
    ctx.fillRect(g.player.x + 20, g.player.y - 10, 10, 10); // turret

    // Aliens - Neon Purple/Pink
    ctx.shadowBlur = 10;
    g.aliens.forEach((alien) => {
      if (!alien.alive) return;
      ctx.fillStyle = alien.type === 0 ? '#ff00ff' : alien.type === 1 ? '#bc13fe' : '#7a00ff';
      ctx.shadowColor = ctx.fillStyle;
      ctx.fillRect(alien.x, alien.y, alien.width, alien.height);
      // Eyes
      ctx.fillStyle = '#fff';
      ctx.fillRect(alien.x + 5, alien.y + 5, 4, 4);
      ctx.fillRect(alien.x + alien.width - 9, alien.y + 5, 4, 4);
    });

    // Projectiles
    g.projectiles.forEach(p => {
      ctx.fillStyle = p.owner === 'player' ? '#00ffcc' : '#ff3333';
      ctx.shadowColor = ctx.fillStyle;
      ctx.fillRect(p.x, p.y, p.width, p.height);
    });

    ctx.shadowBlur = 0;
  };

  return (
    <div className="min-h-screen bg-black text-white flex flex-col items-center justify-center font-mono p-4 overflow-hidden">
      <div 
        className="relative border-4 border-blue-900 rounded-lg shadow-[0_0_50px_rgba(30,58,138,0.5)]"
        data-anvil-state={JSON.stringify({ gameState, score, lives })}
      >
        {/* UI Overlay */}
        <div className="absolute top-4 left-4 right-4 flex justify-between text-xl font-bold z-10 pointer-events-none">
          <div>SCORE: <span className="text-cyan-400">{score}</span></div>
          <div>LIVES: <span className="text-red-500">{'♥'.repeat(lives)}</span></div>
        </div>

        <canvas 
          ref={canvasRef} 
          width={800} 
          height={600} 
          className="bg-slate-950 block max-w-full h-auto"
        />

        {/* Screens */}
        {gameState === 'IDLE' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/80 backdrop-blur-sm z-20">
            <h1 className="text-6xl font-extrabold mb-8 text-transparent bg-clip-text bg-gradient-to-b from-cyan-400 to-blue-600 animate-pulse">
              NEON INVADERS
            </h1>
            <button 
              onClick={resetGame}
              data-anvil-action="primary"
              className="px-8 py-4 bg-cyan-600 hover:bg-cyan-500 text-white rounded-full font-bold text-2xl transition-all transform hover:scale-110 shadow-[0_0_20px_rgba(8,145,178,0.6)]"
            >
              START GAME
            </button>
            <p className="mt-6 text-slate-400">Use Arrow Keys to move & Space to shoot</p>
          </div>
        )}

        {gameState === 'GAMEOVER' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-red-950/80 backdrop-blur-sm z-20">
            <h2 className="text-6xl font-black mb-4 text-white drop-shadow-[0_0_10px_rgba(255,0,0,1)]">GAME OVER</h2>
            <p className="text-2xl mb-8">Final Score: {score}</p>
            <button 
              onClick={resetGame}
              className="px-8 py-4 bg-white text-red-900 rounded-full font-bold text-2xl transition-all hover:bg-slate-200"
            >
              TRY AGAIN
            </button>
          </div>
        )}

        {gameState === 'WON' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-green-950/80 backdrop-blur-sm z-20">
            <h2 className="text-6xl font-black mb-4 text-white drop-shadow-[0_0_10px_rgba(0,255,0,1)]">VICTORY!</h2>
            <p className="text-2xl mb-8">You saved the neon world!</p>
            <button 
              onClick={resetGame}
              className="px-8 py-4 bg-white text-green-900 rounded-full font-bold text-2xl transition-all hover:bg-slate-200"
            >
              PLAY AGAIN
            </button>
          </div>
        )}
      </div>

      <div className="mt-8 text-slate-500 flex gap-4 items-center">
        <span>[←] [→] Move</span>
        <span>[Space] Shoot</span>
        <span>[R] Restart</span>
      </div>
    </div>
  );
}

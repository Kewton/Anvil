"use client";

import React, { useEffect, useRef, useState } from 'react';

interface Entity {
  x: number;
  y: number;
  width: number;
  height: number;
  color: string;
  id: string;
}

interface Alien extends Entity {
  vx: number;
  vy: number;
}

interface Bullet extends Entity {
  vy: number;
}

interface Explosion {
  x: number;
  y: number;
  life: number;
  id: string;
}

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [gameState, setGameState] = useState<'START' | 'PLAYING' | 'GAME_OVER' | 'VICTORY'>('START');
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  
  // Game constants
  const CANVAS_WIDTH = 800;
  const CANVAS_HEIGHT = 600;
  const PLAYER_WIDTH = 50;
  const PLAYER_HEIGHT = 30;
  const ALIEN_WIDTH = 40;
  const ALIEN_HEIGHT = 30;
  const BULLET_WIDTH = 4;
  const BULLET_HEIGHT = 15;

  // Game refs to avoid re-renders in loop
  const gameData = useRef({
    playerX: CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2,
    bullets: [] as Bullet[],
    aliens: [] as Alien[],
    explosions: [] as Explosion[],
    alienDir: 1,
    alienStepDown: false,
    keys: {} as Record<string, boolean>,
    lastShotTime: 0,
  });

  const initAliens = () => {
    const aliens: Alien[] = [];
    for (let row = 0; row < 5; row++) {
      for (let col = 0; col < 10; col++) {
        aliens.push({
          id: `alien-${row}-${col}`,
          x: col * (ALIEN_WIDTH + 20) + 100,
          y: row * (ALIEN_HEIGHT + 20) + 50,
          width: ALIEN_WIDTH,
          height: ALIEN_HEIGHT,
          color: row === 0 ? '#ff00ff' : row < 3 ? '#00ffff' : '#ffff00',
          vx: 2,
          vy: 0,
        });
      }
    }
    gameData.current.aliens = aliens;
  };

  const startGame = () => {
    setScore(0);
    setLives(3);
    setGameState('PLAYING');
    gameData.current.playerX = CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2;
    gameData.current.bullets = [];
    gameData.current.explosions = [];
    initAliens();
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => (gameData.current.keys[e.code] = true);
    const handleKeyUp = (e: KeyboardEvent) => (gameData.current.keys[e.code] = false);
    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
    };
  }, []);

  useEffect(() => {
    if (gameState !== 'PLAYING') return;

    let animationFrameId: number;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const loop = (time: number) => {
      // 1. Input Handling
      if (gameData.current.keys['ArrowLeft'] && gameData.current.playerX > 0) {
        gameData.current.playerX -= 5;
      }
      if (gameData.current.keys['ArrowRight'] && gameData.current.playerX < CANVAS_WIDTH - PLAYER_WIDTH) {
        gameData.current.playerX += 5;
      }
      if (gameData.current.keys['Space'] && time - gameData.current.lastShotTime > 400) {
        gameData.current.bullets.push({
          id: `bullet-${time}`,
          x: gameData.current.playerX + PLAYER_WIDTH / 2 - BULLET_WIDTH / 2,
          y: CANVAS_HEIGHT - PLAYER_HEIGHT - 20,
          width: BULLET_WIDTH,
          height: BULLET_HEIGHT,
          color: '#00ff00',
          vy: -7,
        });
        gameData.current.lastShotTime = time;
      }

      // 2. Alien Movement
      let hitWall = false;
      gameData.current.aliens.forEach(a => {
        a.x += a.vx * gameData.current.alienDir;
        if (a.x <= 0 || a.x >= CANVAS_WIDTH - ALIEN_WIDTH) hitWall = true;
      });

      if (hitWall) {
        gameData.current.alienDir *= -1;
        gameData.current.aliens.forEach(a => {
          a.y += 20;
        });
      }

      // 3. Physics & Collisions
      // Bullets vs Aliens
      gameData.current.bullets = gameData.current.bullets.filter(b => {
        let hit = false;
        gameData.current.aliens = gameData.current.aliens.filter(a => {
          if (
            b.x < a.x + a.width &&
            b.x + b.width > a.x &&
            b.y < a.y + a.height &&
            b.y + b.height > a.y
          ) {
            hit = true;
            gameData.current.explosions.push({
              id: `exp-${Math.random()}`,
              x: a.x + a.width / 2,
              y: a.y + a.height / 2,
              life: 1.0,
            });
            setScore(s => s + 10);
            return false;
          }
          return true;
        });
        if (hit) return false;
        b.y += b.vy;
        return b.y > 0;
      });

      // Alien vs Player (or bottom)
      const alienReachedBottom = gameData.current.aliens.some(a => a.y + a.height >= CANVAS_HEIGHT - PLAYER_HEIGHT - 20);
      if (alienReachedBottom) {
        setGameState('GAME_OVER');
      }

      if (gameData.current.aliens.length === 0) {
        setGameState('VICTORY');
      }

      // Explosions decay
      gameData.current.explosions = gameData.current.explosions.filter(e => {
        e.life -= 0.05;
        return e.life > 0;
      });

      // 4. Rendering
      ctx.clearRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);

      // Player
      ctx.fillStyle = '#00ff00';
      ctx.shadowBlur = 15;
      ctx.shadowColor = '#00ff00';
      ctx.fillRect(gameData.current.playerX, CANVAS_HEIGHT - PLAYER_HEIGHT - 20, PLAYER_WIDTH, PLAYER_HEIGHT);

      // Aliens
      gameData.current.aliens.forEach(a => {
        ctx.fillStyle = a.color;
        ctx.shadowBlur = 10;
        ctx.shadowColor = a.color;
        ctx.fillRect(a.x, a.y, a.width, a.height);
      });

      // Bullets
      gameData.current.bullets.forEach(b => {
        ctx.fillStyle = b.color;
        ctx.shadowBlur = 10;
        ctx.shadowColor = b.color;
        ctx.fillRect(b.x, b.y, b.width, b.height);
      });

      // Explosions
      gameData.current.explosions.forEach(e => {
        ctx.beginPath();
        ctx.arc(e.x, e.y, (1 - e.life) * 30, 0, Math.PI * 2);
        ctx.strokeStyle = `rgba(255, 255, 255, ${e.life})`;
        ctx.lineWidth = 2;
        ctx.stroke();
        ctx.closePath();
      });

      ctx.shadowBlur = 0;

      animationFrameId = requestAnimationFrame(loop);
    };

    animationFrameId = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(animationFrameId);
  }, [gameState]);

  const stateJson = JSON.stringify({ score, lives, gameState });

  return (
    <div className="min-h-screen flex flex-col items-center justify-center bg-black text-white font-mono relative overflow-hidden">
      {/* Background Starfield Decoration */}
      <div className="absolute inset-0 pointer-events-none opacity-50">
        <div className="absolute top-10 left-10 w-1 h-1 bg-white rounded-full animate-pulse" />
        <div className="absolute top-20 right-20 w-2 h-2 bg-white rounded-full animate-ping" />
        <div className="absolute bottom-1/4 left-1/3 w-1 h-1 bg-white rounded-full animate-pulse" />
        <div className="absolute top-1/2 right-1/4 w-1 h-1 bg-white rounded-full animate-ping" />
      </div>

      <div 
        className="relative z-10 flex flex-col items-center"
        data-anvil-state={stateJson}
      >
        <div className="mb-4 flex justify-between w-full max-w-[800px] text-2xl neon-text">
          <div>SCORE: {score}</div>
          <div>LIVES: {lives}</div>
        </div>

        <div className="relative border-4 border-cyan-500 shadow-[0_0_20px_rgba(6,182,212,0.5)] rounded-lg overflow-hidden">
          <canvas 
            ref={canvasRef} 
            width={CANVAS_WIDTH} 
            height={CANVAS_HEIGHT}
            className="bg-slate-900"
          />

          {gameState === 'START' && (
            <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/70 backdrop-blur-sm">
              <h1 className="text-6xl font-black mb-8 neon-text-blue animate-glow">NEON INVADERS</h1>
              <button 
                onClick={startGame}
                data-anvil-action="primary"
                className="px-8 py-4 bg-cyan-500 hover:bg-cyan-400 text-black font-bold rounded-full transition-all hover:scale-110 shadow-[0_0_15px_rgba(6,182,212,1)]"
              >
                LAUNCH MISSION
              </button>
              <p className="mt-6 text-cyan-300 animate-pulse">USE ARROWS TO MOVE & SPACE TO SHOOT</p>
            </div>
          )}

          {gameState === 'GAME_OVER' && (
            <div className="absolute inset-0 flex flex-col items-center justify-center bg-red-900/60 backdrop-blur-sm">
              <h2 className="text-6xl font-black mb-4 neon-text-red">MISSION FAILED</h2>
              <p className="text-2xl mb-8">FINAL SCORE: {score}</p>
              <button 
                onClick={startGame}
                data-anvil-action="restart"
                className="px-8 py-4 bg-white text-red-600 font-bold rounded-full transition-all hover:scale-110 shadow-xl"
              >
                RETRY MISSION
              </button>
            </div>
          )}

          {gameState === 'VICTORY' && (
            <div className="absolute inset-0 flex flex-col items-center justify-center bg-green-900/60 backdrop-blur-sm">
              <h2 className="text-6xl font-black mb-4 neon-text-green">GALAXY SAVED!</h2>
              <p className="text-2xl mb-8">SCORE: {score}</p>
              <button 
                onClick={startGame}
                data-anvil-action="restart"
                className="px-8 py-4 bg-white text-green-600 font-bold rounded-full transition-all hover:scale-110 shadow-xl"
              >
                PLAY AGAIN
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

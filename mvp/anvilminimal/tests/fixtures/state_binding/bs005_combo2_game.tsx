"use client";

import React, { useEffect, useRef, useState } from 'react';

interface Point { x: number; y: number; }
interface Entity extends Point { width: number; height: number; }
interface Enemy extends Entity { type: number; alive: boolean; }
interface Projectile extends Entity { speed: number; owner: 'player' | 'enemy'; }

export default function Game() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [gameState, setGameState] = useState<'MENU' | 'PLAYING' | 'PAUSED' | 'GAMEOVER' | 'VICTORY'>('MENU');
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [wave, setWave] = useState(1);

  // Game Constants
  const CANVAS_WIDTH = 800;
  const CANVAS_HEIGHT = 600;
  const PLAYER_SPEED = 5;
  const BULLET_SPEED = 7;
  const ENEMY_BULLET_SPEED = 4;
  const ENEMY_ROWS = 5;
  const ENEMY_COLS = 11;
  const ENEMY_SPACING = 40;
  const ENEMY_START_Y = 50;

  // Game State Refs (to avoid re-renders in loop)
  const playerRef = useRef<Entity>({ x: CANVAS_WIDTH / 2 - 20, y: CANVAS_HEIGHT - 60, width: 40, height: 30 });
  const enemiesRef = useRef<Enemy[]>([]);
  const bulletsRef = useRef<Projectile[]>([]);
  const keysPressed = useRef<Set<string>>(new Set());
  const enemyDir = useRef(1);
  const enemyStepDown = useRef(false);
  const lastEnemyShotTime = useRef(0);

  const initEnemies = () => {
    const enemies: Enemy[] = [];
    for (let row = 0; row < ENEMY_ROWS; row++) {
      for (let col = 0; col < ENEMY_COLS; col++) {
        enemies.push({
          x: col * ENEMY_SPACING + 50,
          y: row * ENEMY_SPACING + ENEMY_START_Y,
          width: 30,
          height: 20,
          type: row === 0 ? 3 : row < 3 ? 2 : 1,
          alive: true,
        });
      }
    }
    enemiesRef.current = enemies;
  };

  const spawnEnemyBullet = (enemy: Enemy) => {
    bulletsRef.current.push({
      x: enemy.x + enemy.width / 2,
      y: enemy.y + enemy.height,
      width: 4,
      height: 10,
      speed: -ENEMY_BULLET_SPEED,
      owner: 'enemy',
    });
  };

  const restartGame = () => {
    setScore(0);
    setLives(3);
    setWave(1);
    playerRef.current = { x: CANVAS_WIDTH / 2 - 20, y: CANVAS_HEIGHT - 60, width: 40, height: 30 };
    bulletsRef.current = [];
    initEnemies();
    setGameState('PLAYING');
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      keysPressed.current.add(e.code);
      if (e.code === 'KeyP') {
        setGameState(prev => prev === 'PLAYING' ? 'PAUSED' : prev === 'PAUSED' ? 'PLAYING' : prev);
      }
      if (e.code === 'KeyR') {
        restartGame();
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      keysPressed.current.delete(e.code);
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

    let animationFrameId: number;

    const gameLoop = (time: number) => {
      // 1. Input Handling
      if (keysPressed.current.has('ArrowLeft') || keysPressed.current.has('KeyA')) {
        playerRef.current.x = Math.max(0, playerRef.current.x - PLAYER_SPEED);
      }
      if (keysPressed.current.has('ArrowRight') || keysPressed.current.has('KeyD')) {
        playerRef.current.x = Math.min(CANVAS_WIDTH - playerRef.current.width, playerRef.current.x + PLAYER_SPEED);
      }
      if (keysPressed.current.has('Space')) {
        const currentBullets = bulletsRef.current;
        const playerBullets = currentBullets.filter(b => b.owner === 'player');
        if (playerBullets.length < 3) {
          bulletsRef.current = [...currentBullets, {
            x: playerRef.current.x + playerRef.current.width / 2 - 2,
            y: playerRef.current.y,
            width: 4,
            height: 10,
            speed: BULLET_SPEED,
            owner: 'player'
          }];
          keysPressed.current.delete('Space'); // Semi-auto firing
        }
      }

      // 2. Enemy Movement
      let moveDown = false;
      const aliveEnemies = enemiesRef.current.filter(e => e.alive);
      
      if (aliveEnemies.length === 0) {
        setWave(w => w + 1);
        initEnemies();
        return;
      }

      aliveEnemies.forEach(e => {
        e.x += enemyDir.current * 1.5;
        if (e.x + e.width > CANVAS_WIDTH || e.x < 0) {
          moveDown = true;
        }
      });

      if (moveDown) {
        enemyDir.current *= -1;
        enemiesRef.current.forEach(e => { e.y += 20; });
      }

      // 3. Enemy Shooting
      if (time - lastEnemyShotTime.current > 1000) {
        const shooter = aliveEnemies[Math.floor(Math.random() * aliveEnemies.length)];
        if (shooter) spawnEnemyBullet(shooter);
        lastEnemyShotTime.current = time;
      }

      // 4. Projectiles Update
      bulletsRef.current = bulletsRef.current.filter(b => {
        if (b.owner === 'player') {
          b.y -= BULLET_SPEED;
        } else {
          b.y += ENEMY_BULLET_SPEED;
        }
        return b.y > 0 && b.y < CANVAS_HEIGHT;
      });

      // 5. Collision Detection
      bulletsRef.current = bulletsRef.current.filter(b => {
        if (b.owner === 'player') {
          for (const e of enemiesRef.current) {
            if (e.alive && b.x < e.x + e.width && b.x + b.width > e.x && b.y < e.y + e.height && b.y + b.height > e.y) {
              e.alive = false;
              setScore(s => s + (e.type * 10));
              return false;
            }
          }
        } else {
          if (b.x < playerRef.current.x + playerRef.current.width && b.x + b.width > playerRef.current.x && b.y < playerRef.current.y + playerRef.current.height && b.y + b.height > playerRef.current.y) {
            setLives(l => {
              if (l <= 1) setGameState('GAMEOVER');
              return l - 1;
            });
            return false;
          }
        }
        return true;
      });

      // Check if enemies reached the player
      const reached = enemiesRef.current.some(e => e.alive && e.y + e.height >= playerRef.current.y);
      if (reached) {
        setGameState('GAMEOVER');
      }

      // 6. Rendering
      ctx.fillStyle = '#000';
      ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);

      // Player
      ctx.fillStyle = '#0f0';
      ctx.fillRect(playerRef.current.x, playerRef.current.y, playerRef.current.width, playerRef.current.height);
      ctx.fillRect(playerRef.current.x + 15, playerRef.current.y - 10, 10, 10);

      // Enemies
      enemiesRef.current.forEach(e => {
        if (!e.alive) return;
        ctx.fillStyle = e.type === 3 ? '#f0f' : e.type === 2 ? '#0ff' : '#fff';
        ctx.fillRect(e.x, e.y, e.width, e.height);
        ctx.fillStyle = '#000';
        ctx.fillRect(e.x + 5, e.y + 5, 4, 4);
        ctx.fillRect(e.x + e.width - 9, e.y + 5, 4, 4);
      });

      // Bullets
      bulletsRef.current.forEach(b => {
        ctx.fillStyle = b.owner === 'player' ? '#fff' : '#f00';
        ctx.fillRect(b.x, b.y, b.width, b.height);
      });

      animationFrameId = requestAnimationFrame(gameLoop);
    };

    animationFrameId = requestAnimationFrame(gameLoop);
    return () => cancelAnimationFrame(animationFrameId);
  }, [gameState]);

  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-slate-900 text-white font-mono p-4">
      <div className="mb-4 flex justify-between w-[800px] text-2xl font-bold uppercase tracking-widest">
        <div>Score: {score}</div>
        <div>Lives: {lives}</div>
        <div>Wave: {wave}</div>
      </div>
      
      <div className="relative border-4 border-slate-700 shadow-2xl">
        <canvas 
          ref={canvasRef} 
          width={CANVAS_WIDTH} 
          height={CANVAS_HEIGHT} 
          className="bg-black"
        />
        
        {gameState === 'MENU' && (
          <div className="absolute inset-0 bg-black/80 flex flex-col items-center justify-center gap-6">
            <h1 className="text-6xl font-black italic text-transparent bg-clip-text bg-gradient-to-r from-green-400 to-blue-500">
              SPACE INVADERS
            </h1>
            <p className="text-slate-400">Arrow Keys to Move | Space to Shoot</p>
            <button 
              data-anvil-action="primary"
              onClick={restartGame}
              className="px-8 py-3 bg-green-500 hover:bg-green-400 text-black font-bold rounded-full transition-all transform hover:scale-110"
            >
              START MISSION
            </button>
          </div>
        )}

        {gameState === 'PAUSED' && (
          <div className="absolute inset-0 bg-black/50 flex flex-col items-center justify-center gap-4">
            <h2 className="text-5xl font-bold">PAUSED</h2>
            <button 
              data-anvil-action="primary"
              onClick={() => setGameState('PLAYING')}
              className="px-6 py-2 bg-white text-black font-bold rounded-md"
            >
              RESUME
            </button>
            <p className="text-sm">Press P to unpause</p>
          </div>
        )}

        {gameState === 'GAMEOVER' && (
          <div className="absolute inset-0 bg-red-900/80 flex flex-col items-center justify-center gap-6">
            <h2 className="text-6xl font-black text-white">MISSION FAILED</h2>
            <p className="text-2xl">Final Score: {score}</p>
            <button 
              data-anvil-action="restart"
              onClick={restartGame}
              className="px-8 py-3 bg-white text-red-900 font-bold rounded-full transition-all transform hover:scale-110"
            >
              RETRY MISSION
            </button>
            <p className="text-sm opacity-70">Press R to restart quickly</p>
          </div>
        )}

        {gameState === 'VICTORY' && (
          <div className="absolute inset-0 bg-green-900/80 flex flex-col items-center justify-center gap-6">
            <h2 className="text-6xl font-black text-white">GALAXY SAVED!</h2>
            <p className="text-2xl">Final Score: {score}</p>
            <button 
              data-anvil-action="restart"
              onClick={restartGame}
              className="px-8 py-3 bg-white text-green-900 font-bold rounded-full transition-all transform hover:scale-110"
            >
              PLAY AGAIN
            </button>
          </div>
        )}
      </div>
      
      <div 
        data-anvil-state={JSON.stringify({ score, lives, wave, gameState })}
        className="mt-4 text-slate-500 text-xs"
      >
        System Status: {gameState} | Wave: {wave}
      </div>
    </div>
  );
}

"use client";

import React, { useEffect, useRef, useState, useCallback } from 'react';

type GameState = 'START' | 'PLAYING' | 'GAME_OVER' | 'VICTORY';

interface Entity {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Bullet extends Entity {
  vx: number;
  vy: number;
}

interface Enemy extends Entity {
  type: number; // 0: small, 1: medium, 2: large
  alive: boolean;
}

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [gameState, setGameState] = useState<GameState>('START');
  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [level, setLevel] = useState(1);
  const [enemiesRemaining, setEnemiesRemaining] = useState(0);

  // Game constants
  const CANVAS_WIDTH = 800;
  const CANVAS_HEIGHT = 600;
  const PLAYER_WIDTH = 40;
  const PLAYER_HEIGHT = 20;
  const ENEMY_WIDTH = 30;
  const ENEMY_HEIGHT = 20;
  const BULLET_WIDTH = 4;
  const BULLET_HEIGHT = 12;
  const ENEMY_ROWS = 5;
  const ENEMY_COLS = 11;
  const ENEMY_SPACING_X = 15;
  const ENEMY_SPACING_Y = 15;

  // Ref-based game state for the loop
  const gameRef = useRef({
    player: { x: CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2, y: CANVAS_HEIGHT - 50 },
    bullets: [] as Bullet[],
    enemyBullets: [] as Bullet[],
    enemies: [] as Enemy[],
    enemyDirection: 1,
    enemyStepDown: false,
    enemySpeed: 1,
    keys: {} as Record<string, boolean>,
    lastShotTime: 0,
    shotCooldown: 400,
  });

  const initEnemies = useCallback((lvl: number) => {
    const enemies: Enemy[] = [];
    for (let row = 0; row < ENEMY_ROWS; row++) {
      for (let col = 0; col < ENEMY_COLS; col++) {
        enemies.push({
          x: col * (ENEMY_WIDTH + ENEMY_SPACING_X) + 50,
          y: row * (ENEMY_HEIGHT + ENEMY_SPACING_Y) + 50,
          width: ENEMY_WIDTH,
          height: ENEMY_HEIGHT,
          type: Math.floor(row / 2),
          alive: true,
        });
      }
    }
    setEnemiesRemaining(enemies.length);
    gameRef.current.enemies = enemies;
    gameRef.current.enemySpeed = 0.5 + (lvl * 0.2);
  }, []);

  const startGame = () => {
    setScore(0);
    setLives(3);
    setLevel(1);
    initEnemies(1);
    gameRef.current.player = { x: CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2, y: CANVAS_HEIGHT - 50 };
    gameRef.current.bullets = [];
    gameRef.current.enemyBullets = [];
    setGameState('PLAYING');
  };

  const restartGame = () => {
    startGame();
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      gameRef.current.keys[e.code] = true;
      if (e.code === 'KeyR' && gameState !== 'START') {
        restartGame();
      }
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
  }, [gameState]);

  useEffect(() => {
    if (gameState !== 'PLAYING') return;

    let animationFrameId: number;
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const loop = (time: number) => {
      update(time);
      draw(ctx);
      animationFrameId = requestAnimationFrame(loop);
    };

    const update = (time: number) => {
      const { player, bullets, enemyBullets, enemies, keys } = gameRef.current;

      // Player movement
      if ((keys['ArrowLeft'] || keys['KeyA']) && player.x > 0) {
        player.x -= 5;
      }
      if ((keys['ArrowRight'] || keys['KeyD']) && player.x < CANVAS_WIDTH - PLAYER_WIDTH) {
        player.x += 5;
      }

      // Player shooting
      if (keys['Space'] && time - gameRef.current.lastShotTime > gameRef.current.shotCooldown) {
        bullets.push({
          x: player.x + PLAYER_WIDTH / 2 - BULLET_WIDTH / 2,
          y: player.y,
          width: BULLET_WIDTH,
          height: BULLET_HEIGHT,
          vx: 0,
          vy: -7,
        });
        gameRef.current.lastShotTime = time;
      }

      // Bullet updates
      gameRef.current.bullets = bullets.filter(b => {
        b.y += b.vy;
        return b.y + b.height > 0;
      });

      gameRef.current.enemyBullets = enemyBullets.filter(b => {
        b.y += b.vy;
        return b.y + b.height < CANVAS_HEIGHT;
      });

      // Enemy movement logic
      let shouldStepDown = false;
      let allAlive = false;
      
      // Determine if any enemy is hitting the boundary
      enemies.forEach(e => {
        if (!e.alive) return;
        allAlive = true;
        if (gameRef.current.enemyDirection === 1 && e.x + e.width > CANVAS_WIDTH - 20) {
          shouldStepDown = true;
        } else if (gameRef.current.enemyDirection === -1 && e.x < 20) {
          shouldStepDown = true;
        }
      });

      if (shouldStepDown) {
        gameRef.current.enemyDirection *= -1;
        enemies.forEach(e => { if (e.alive) e.y += 20; });
      }

      enemies.forEach(e => {
        if (!e.alive) return;
        e.x += gameRef.current.enemyDirection * gameRef.current.enemySpeed;

        // Random shooting
        if (Math.random() < 0.001 * level) {
          gameRef.current.enemyBullets.push({
            x: e.x + e.width / 2,
            y: e.y + e.height,
            width: BULLET_WIDTH,
            height: BULLET_HEIGHT,
            vx: 0,
            vy: 4,
          });
        }
      });

      // Collision: Player Bullets -> Enemies
      bullets.forEach(b => {
        enemies.forEach(e => {
          if (e.alive && b.x < e.x + e.width && b.x + b.width > e.x && b.y < e.y + e.height && b.y + b.height > e.y) {
            e.alive = false;
            b.y = -100; // Mark for removal
            const points = (e.type + 1) * 10;
            setScore(s => s + points);
            setEnemiesRemaining(prev => prev - 1);
          }
        });
      });

      // Collision: Enemy Bullets -> Player
      enemyBullets.forEach(eb => {
        if (eb.x < player.x + PLAYER_WIDTH && eb.x + eb.width > player.x && eb.y < player.y + PLAYER_HEIGHT && eb.y + eb.height > player.y) {
          eb.y = CANVAS_HEIGHT + 100; // Mark for removal
          setLives(l => {
            const next = l - 1;
            if (next <= 0) setGameState('GAME_OVER');
            return next;
          });
        }
      });

      // Game Over check (enemies reached player)
      enemies.forEach(e => {
        if (e.alive && e.y + e.height >= player.y) {
          setGameState('GAME_OVER');
        }
      });

      // Victory check
      const remaining = enemies.filter(e => e.alive).length;
      if (remaining === 0) {
        setGameState('VICTORY');
      }
    };

    const draw = (ctx: CanvasRenderingContext2D) => {
      ctx.fillStyle = '#000';
      ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);

      // Player
      ctx.fillStyle = '#0ff';
      ctx.shadowBlur = 15;
      ctx.shadowColor = '#0ff';
      ctx.fillRect(gameRef.current.player.x, gameRef.current.player.y, PLAYER_WIDTH, PLAYER_HEIGHT);

      // Player Bullets
      ctx.fillStyle = '#fff';
      ctx.shadowColor = '#fff';
      gameRef.current.bullets.forEach(b => {
        ctx.fillRect(b.x, b.y, b.width, b.height);
      });

      // Enemy Bullets
      ctx.fillStyle = '#f0f';
      ctx.shadowColor = '#f0f';
      gameRef.current.enemyBullets.forEach(b => {
        ctx.fillRect(b.x, b.y, b.width, b.height);
      });

      // Enemies
      gameRef.current.enemies.forEach(e => {
        if (!e.alive) return;
        const colors = ['#0f0', '#ff0', '#f00'];
        ctx.fillStyle = colors[e.type];
        ctx.shadowColor = colors[e.type];
        ctx.fillRect(e.x, e.y, e.width, e.height);
      });
      ctx.shadowBlur = 0;
    };

    animationFrameId = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(animationFrameId);
  }, [gameState, level, initEnemies]);

  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-black text-white font-mono overflow-hidden">
      <div className="relative border-4 border-cyan-500 shadow-[0_0_20px_rgba(0,255,255,0.5)]">
        <div className="absolute top-4 left-4 right-4 flex justify-between text-xl z-10 pointer-events-none">
          <div className="text-cyan-400">SCORE: {score}</div>
          <div className="text-pink-400">LIVES: {lives}</div>
          <div className="text-yellow-400">LEVEL: {level}</div>
        </div>
        
        <canvas
          ref={canvasRef}
          width={CANVAS_WIDTH}
          height={CANVAS_HEIGHT}
          className="block bg-black"
        />

        {gameState === 'START' && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/80 z-20">
            <h1 className="text-6xl font-bold mb-8 text-transparent bg-clip-text bg-gradient-to-b from-cyan-400 to-blue-600 animate-pulse">
              SPACE INVADERS
            </h1>
            <button
              data-anvil-action="primary"
              onClick={startGame}
              className="px-8 py-4 text-2xl bg-transparent border-2 border-cyan-400 text-cyan-400 hover:bg-cyan-400 hover:text-black transition-all duration-300 shadow-[0_0_15px_rgba(0,255,255,0.8)]"
            >
              START MISSION
            </button>
            <p className="mt-6 text-gray-400">Arrows/AD to Move • Space to Shoot</p>
          </div>
        )}

        {(gameState === 'GAME_OVER' || gameState === 'VICTORY') && (
          <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/80 z-20">
            <h2 className={`text-6xl font-bold mb-8 ${gameState === 'VICTORY' ? 'text-yellow-400' : 'text-red-500'}`}>
              {gameState === 'VICTORY' ? 'MISSION ACCOMPLISHED' : 'GAME OVER'}
            </h2>
            <div className="text-3xl mb-8">FINAL SCORE: {score}</div>
            <button
              data-anvil-action="restart"
              onClick={restartGame}
              className="px-8 py-4 text-2xl bg-transparent border-2 border-pink-500 text-pink-500 hover:bg-pink-500 hover:text-white transition-all duration-300 shadow-[0_0_15px_rgba(255,0,255,0.8)]"
            >
              TRY AGAIN
            </button>
          </div>
        )}
      </div>
      
      <div 
        className="mt-4 text-xs text-gray-600" 
        data-anvil-state={JSON.stringify({
          score,
          lives,
          level,
          gameState,
          enemiesRemaining
        })}
      >
        Game State Engine Active
      </div>
    </div>
  );
}

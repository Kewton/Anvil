'use client';

import React, { useRef, useEffect, useState, useCallback } from 'react';

// --- Types ---
interface Vec2 {
  x: number;
  y: number;
}

interface Entity {
  x: number;
  y: number;
  width: number;
  height: number;
  color: string;
  alive: boolean;
}

interface Projectile extends Entity {
  vy: number;
  isPlayer: boolean;
}

interface Alien extends Entity {
  type: number; // 0, 1, 2 for different alien types
  row: number;
  col: number;
  points: number;
}

interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  maxLife: number;
  color: string;
  size: number;
}

interface Star {
  x: number;
  y: number;
  speed: number;
  brightness: number;
  size: number;
}

type GameState = 'menu' | 'playing' | 'gameover' | 'paused';

// --- Constants ---
const CANVAS_WIDTH = 640;
const CANVAS_HEIGHT = 720;
const PLAYER_WIDTH = 40;
const PLAYER_HEIGHT = 30;
const PLAYER_SPEED = 5;
const BULLET_SPEED = 8;
const ALIEN_BULLET_SPEED = 3;
const ALIEN_COLS = 11;
const ALIEN_ROWS = 5;
const ALIEN_WIDTH = 32;
const ALIEN_HEIGHT = 24;
const ALIEN_PADDING = 8;
const ALIEN_TOP_OFFSET = 60;
const ALIEN_MOVE_INTERVAL_BASE = 800;
const ALIEN_SHOOT_CHANCE = 0.003;
const MAX_BULLETS = 3;
const PLAYER_LIVES = 3;
const UFO_POINTS = [30, 20, 10];
const UFO_COLORS = ['#ff6ec7', '#ffff00', '#39ff14'];
const ALIEN_SPRITES = [
  // Type 0 (top row) - Squid
  [
    '  #  #  ',
    ' ## ## ##',
    '#########',
    '# #  # # ',
    '###  ### ',
  ],
  // Type 1 (middle rows) - Crab
  [
    '#      #',
    ' #    # ',
    '###  ###',
    '# #  # #',
    ' #    # ',
  ],
  // Type 2 (bottom rows) - Octopus
  [
    '  ###  ',
    ' ##### ',
    '#######',
    '# # # #',
    '##   ##',
  ],
];

const UFO_SPRITE = [
  '   ####  ',
  ' ##  ##  ',
  '#########',
  '# # # # #',
];

export default function SpaceInvaders() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const gameStateRef = useRef<GameState>('menu');
  const [gameState, setGameState] = useState<GameState>('menu');
  const [displayState, setDisplayState] = useState({
    score: 0,
    lives: PLAYER_LIVES,
    level: 1,
    gameOver: false,
    phase: 'menu' as string,
    aliensRemaining: 0,
    highScore: 0,
  });

  // Game refs for mutable state in game loop
  const playerRef = useRef<Entity>({
    x: CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2,
    y: CANVAS_HEIGHT - 60,
    width: PLAYER_WIDTH,
    height: PLAYER_HEIGHT,
    color: '#39ff14',
    alive: true,
  });
  const aliensRef = useRef<Alien[]>([]);
  const bulletsRef = useRef<Projectile[]>([]);
  const alienBulletsRef = useRef<Projectile[]>([]);
  const particlesRef = useRef<Particle[]>([]);
  const starsRef = useRef<Star[]>([]);
  const keysRef = useRef<Set<string>>(new Set());
  const alienDirRef = useRef<number>(1);
  const alienMoveTimerRef = useRef<number>(0);
  const scoreRef = useRef<number>(0);
  const livesRef = useRef<number>(PLAYER_LIVES);
  const levelRef = useRef<number>(1);
  const highScoreRef = useRef<number>(0);
  const ufoRef = useRef<Entity & { active: boolean; x: number; dir: number }>({
    x: 0,
    y: 30,
    width: 48,
    height: 20,
    color: '#ff6600',
    alive: true,
    active: false,
    dir: 1,
  });
  const ufoTimerRef = useRef<number>(0);
  const lastTimeRef = useRef<number>(0);
  const animFrameRef = useRef<number>(0);
  const shakeTimerRef = useRef<number>(0);
  const flashTimerRef = useRef<number>(0);
  const comboRef = useRef<number>(0);
  const comboTimerRef = useRef<number>(0);

  // --- Initialize stars ---
  const initStars = useCallback(() => {
    const stars: Star[] = [];
    for (let i = 0; i < 100; i++) {
      stars.push({
        x: Math.random() * CANVAS_WIDTH,
        y: Math.random() * CANVAS_HEIGHT,
        speed: 0.2 + Math.random() * 0.8,
        brightness: 0.3 + Math.random() * 0.7,
        size: Math.random() > 0.9 ? 2 : 1,
      });
    }
    starsRef.current = stars;
  }, []);

  // --- Initialize aliens ---
  const initAliens = useCallback(() => {
    const aliens: Alien[] = [];
    const totalWidth = ALIEN_COLS * (ALIEN_WIDTH + ALIEN_PADDING);
    const startX = (CANVAS_WIDTH - totalWidth) / 2;

    for (let row = 0; row < ALIEN_ROWS; row++) {
      for (let col = 0; col < ALIEN_COLS; col++) {
        const type = row === 0 ? 0 : row < 3 ? 1 : 2;
        aliens.push({
          x: startX + col * (ALIEN_WIDTH + ALIEN_PADDING),
          y: ALIEN_TOP_OFFSET + row * (ALIEN_HEIGHT + ALIEN_PADDING),
          width: ALIEN_WIDTH,
          height: ALIEN_HEIGHT,
          color: UFO_COLORS[type],
          alive: true,
          type,
          row,
          col,
          points: (ALIEN_ROWS - row) * 10,
        });
      }
    }
    aliensRef.current = aliens;
  }, []);

  // --- Explosion particles ---
  const spawnExplosion = useCallback((x: number, y: number, color: string, count: number = 12) => {
    const particles: Particle[] = [];
    for (let i = 0; i < count; i++) {
      const angle = (Math.PI * 2 * i) / count + Math.random() * 0.5;
      const speed = 1 + Math.random() * 3;
      particles.push({
        x,
        y,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed,
        life: 30 + Math.random() * 20,
        maxLife: 50,
        color,
        size: 2 + Math.random() * 3,
      });
    }
    particlesRef.current.push(...particles);
  }, []);

  // --- Draw sprite from character map ---
  const drawSprite = useCallback((ctx: CanvasRenderingContext2D, sprite: string[], x: number, y: number, color: string, scale: number = 2) => {
    ctx.fillStyle = color;
    for (let row = 0; row < sprite.length; row++) {
      for (let col = 0; col < sprite[row].length; col++) {
        if (sprite[row][col] === '#') {
          ctx.fillRect(x + col * scale, y + row * scale, scale, scale);
        }
      }
    }
  }, []);

  // --- Draw player ship ---
  const drawPlayer = useCallback((ctx: CanvasRenderingContext2D, player: Entity) => {
    ctx.fillStyle = player.color;
    // Ship body
    ctx.fillRect(player.x + 12, player.y, player.width - 24, player.height);
    ctx.fillRect(player.x + 8, player.y + 8, player.width - 16, player.height - 8);
    ctx.fillRect(player.x + 4, player.y + 16, player.width - 8, player.height - 16);
    // Cockpit
    ctx.fillStyle = '#00ffff';
    ctx.fillRect(player.x + 16, player.y + 4, 8, 8);
    // Thruster glow
    ctx.fillStyle = `rgba(255, 100, 0, ${0.5 + Math.random() * 0.5})`;
    ctx.fillRect(player.x + 14, player.y + player.height - 4, 4, 4 + Math.random() * 4);
    ctx.fillRect(player.x + 22, player.y + player.height - 4, 4, 4 + Math.random() * 4);
  }, []);

  // --- Draw UFO ---
  const drawUfo = useCallback((ctx: CanvasRenderingContext2D, ufo: Entity) => {
    const sprite = UFO_SPRITE;
    drawSprite(ctx, sprite, ufo.x, ufo.y, ufo.color, 2);
    // Glow effect
    ctx.shadowColor = ufo.color;
    ctx.shadowBlur = 10;
    ctx.fillStyle = 'transparent';
    ctx.fillRect(ufo.x, ufo.y, ufo.width, ufo.height);
    ctx.shadowBlur = 0;
  }, [drawSprite]);

  // --- AABB collision ---
  const checkCollision = (a: Entity, b: Entity): boolean => {
    return (
      a.x < b.x + b.width &&
      a.x + a.width > b.x &&
      a.y < b.y + b.height &&
      a.y + a.height > b.y
    );
  };

  // --- Game loop ---
  const gameLoop = useCallback((timestamp: number) => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dt = lastTimeRef.current ? timestamp - lastTimeRef.current : 16;
    lastTimeRef.current = timestamp;

    const state = gameStateRef.current;

    // --- Update ---
    if (state === 'playing') {
      const keys = keysRef.current;
      const player = playerRef.current;

      // Player movement
      if (keys.has('ArrowLeft') || keys.has('a')) {
        player.x = Math.max(0, player.x - PLAYER_SPEED);
      }
      if (keys.has('ArrowRight') || keys.has('d')) {
        player.x = Math.min(CANVAS_WIDTH - player.width, player.x + PLAYER_SPEED);
      }

      // Combo timer
      if (comboTimerRef.current > 0) {
        comboTimerRef.current -= dt;
        if (comboTimerRef.current <= 0) {
          comboRef.current = 0;
        }
      }

      // Alien movement
      alienMoveTimerRef.current += dt;
      const aliveAliens = aliensRef.current.filter((a) => a.alive);
      const moveInterval = Math.max(100, ALIEN_MOVE_INTERVAL_BASE - (ALIEN_ROWS * ALIEN_COLS - aliveAliens.length) * 15);

      if (alienMoveTimerRef.current >= moveInterval) {
        alienMoveTimerRef.current = 0;
        let hitEdge = false;
        const aliens = aliensRef.current;

        // Check edge collision
        for (const alien of aliens) {
          if (!alien.alive) continue;
          if (alienDirRef.current > 0 && alien.x + alien.width >= CANVAS_WIDTH - 10) {
            hitEdge = true;
            break;
          }
          if (alienDirRef.current < 0 && alien.x <= 10) {
            hitEdge = true;
            break;
          }
        }

        if (hitEdge) {
          alienDirRef.current *= -1;
          for (const alien of aliens) {
            if (alien.alive) alien.y += 12;
          }
        } else {
          for (const alien of aliens) {
            if (alien.alive) alien.x += alienDirRef.current * 8;
          }
        }
      }

      // Alien shooting
      for (const alien of aliveAliens) {
        if (Math.random() < ALIEN_SHOOT_CHANCE * (1 + levelRef.current * 0.2)) {
          if (alienBulletsRef.current.length < 5) {
            alienBulletsRef.current.push({
              x: alien.x + alien.width / 2 - 2,
              y: alien.y + alien.height,
              width: 4,
              height: 10,
              color: '#ff6ec7',
              alive: true,
              vy: ALIEN_BULLET_SPEED,
              isPlayer: false,
            });
          }
          break; // Only one alien shoots per frame
        }
      }

      // UFO logic
      ufoTimerRef.current += dt;
      const ufo = ufoRef.current;
      if (!ufo.active && ufoTimerRef.current > 5000 + Math.random() * 10000) {
        ufo.active = true;
        ufo.x = Math.random() > 0.5 ? -50 : CANVAS_WIDTH;
        ufo.dir = ufo.x < 0 ? 1 : -1;
        ufoTimerRef.current = 0;
      }
      if (ufo.active) {
        ufo.x += ufo.dir * 2;
        if (ufo.x > CANVAS_WIDTH + 50 || ufo.x < -60) {
          ufo.active = false;
        }
      }

      // Update bullets
      for (const bullet of bulletsRef.current) {
        bullet.y += bullet.vy;
        if (bullet.y < 0) bullet.alive = false;
      }
      for (const bullet of alienBulletsRef.current) {
        bullet.y += bullet.vy;
        if (bullet.y > CANVAS_HEIGHT) bullet.alive = false;
      }

      // Update particles
      for (const p of particlesRef.current) {
        p.x += p.vx;
        p.y += p.vy;
        p.vy += 0.05; // gravity
        p.life -= 1;
      }
      particlesRef.current = particlesRef.current.filter((p) => p.life > 0);

      // --- Collision detection ---
      // Player bullets vs aliens
      for (const bullet of bulletsRef.current) {
        if (!bullet.alive) continue;
        for (const alien of aliensRef.current) {
          if (!alien.alive) continue;
          if (checkCollision(bullet, alien)) {
            bullet.alive = false;
            alien.alive = false;
            comboRef.current += 1;
            comboTimerRef.current = 2000;
            const comboMultiplier = Math.min(comboRef.current, 10);
            const points = alien.points * comboMultiplier;
            scoreRef.current += points;
            spawnExplosion(alien.x + alien.width / 2, alien.y + alien.height / 2, alien.color, 15);
            shakeTimerRef.current = 5;
            break;
          }
        }
        // Player bullets vs UFO
        if (bullet.alive && ufo.active && checkCollision(bullet, ufo)) {
          bullet.alive = false;
          ufo.active = false;
          scoreRef.current += 300;
          spawnExplosion(ufo.x + ufo.width / 2, ufo.y + ufo.height / 2, '#ff6600', 25);
          shakeTimerRef.current = 10;
          flashTimerRef.current = 10;
        }
      }

      // Alien bullets vs player
      for (const bullet of alienBulletsRef.current) {
        if (!bullet.alive) continue;
        if (checkCollision(bullet, playerRef.current)) {
          bullet.alive = false;
          livesRef.current -= 1;
          comboRef.current = 0;
          spawnExplosion(player.x + player.width / 2, player.y + player.height / 2, '#39ff14', 20);
          shakeTimerRef.current = 15;
          flashTimerRef.current = 15;

          if (livesRef.current <= 0) {
            gameStateRef.current = 'gameover';
            if (scoreRef.current > highScoreRef.current) {
              highScoreRef.current = scoreRef.current;
            }
          } else {
            // Reset player position
            playerRef.current.x = CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2;
          }
          break;
        }
      }

      // Aliens reaching player level
      for (const alien of aliensRef.current) {
        if (alien.alive && alien.y + alien.height >= playerRef.current.y) {
          gameStateRef.current = 'gameover';
          if (scoreRef.current > highScoreRef.current) {
            highScoreRef.current = scoreRef.current;
          }
          break;
        }
      }

      // Check wave clear
      if (aliveAliens.length === 0) {
        levelRef.current += 1;
        initAliens();
        bulletsRef.current = [];
        alienBulletsRef.current = [];
        // Bonus for level clear
        scoreRef.current += 1000;
        flashTimerRef.current = 20;
      }

      // Clean up dead entities
      bulletsRef.current = bulletsRef.current.filter((b) => b.alive);
      alienBulletsRef.current = alienBulletsRef.current.filter((b) => b.alive);
    }

    // Update stars
    for (const star of starsRef.current) {
      star.y += star.speed;
      if (star.y > CANVAS_HEIGHT) {
        star.y = 0;
        star.x = Math.random() * CANVAS_WIDTH;
      }
    }

    // Timers
    if (shakeTimerRef.current > 0) shakeTimerRef.current -= 1;
    if (flashTimerRef.current > 0) flashTimerRef.current -= 1;

    // --- Render ---
    ctx.save();

    // Screen shake
    if (shakeTimerRef.current > 0) {
      const shakeX = (Math.random() - 0.5) * shakeTimerRef.current * 2;
      const shakeY = (Math.random() - 0.5) * shakeTimerRef.current * 2;
      ctx.translate(shakeX, shakeY);
    }

    // Clear
    ctx.fillStyle = '#0a0a1a';
    ctx.fillRect(-10, -10, CANVAS_WIDTH + 20, CANVAS_HEIGHT + 20);

    // Draw stars
    for (const star of starsRef.current) {
      ctx.fillStyle = `rgba(255, 255, 255, ${star.brightness})`;
      ctx.fillRect(star.x, star.y, star.size, star.size);
    }

    if (state === 'playing' || state === 'paused') {
      // Draw aliens
      for (const alien of aliensRef.current) {
        if (!alien.alive) continue;
        const sprite = ALIEN_SPRITES[alien.type];
        // Animate aliens
        const frame = Math.floor(timestamp / 500) % 2;
        const offsetY = frame * 2;
        drawSprite(ctx, sprite, alien.x, alien.y + offsetY, alien.color, 2);
      }

      // Draw UFO
      if (ufoRef.current.active) {
        drawUfo(ctx, ufoRef.current);
      }

      // Draw player
      if (livesRef.current > 0) {
        drawPlayer(ctx, playerRef.current);
      }

      // Draw player bullets
      for (const bullet of bulletsRef.current) {
        if (!bullet.alive) continue;
        ctx.fillStyle = '#39ff14';
        ctx.shadowColor = '#39ff14';
        ctx.shadowBlur = 6;
        ctx.fillRect(bullet.x, bullet.y, bullet.width, bullet.height);
        ctx.shadowBlur = 0;
      }

      // Draw alien bullets
      for (const bullet of alienBulletsRef.current) {
        if (!bullet.alive) continue;
        ctx.fillStyle = '#ff6ec7';
        ctx.shadowColor = '#ff6ec7';
        ctx.shadowBlur = 6;
        // Zigzag bullet shape
        ctx.fillRect(bullet.x, bullet.y, bullet.width, bullet.height);
        ctx.shadowBlur = 0;
      }

      // Draw particles
      for (const p of particlesRef.current) {
        const alpha = p.life / p.maxLife;
        ctx.fillStyle = p.color.replace(')', `, ${alpha})`).replace('rgb', 'rgba');
        if (!p.color.includes('rgba')) {
          ctx.globalAlpha = alpha;
          ctx.fillStyle = p.color;
        }
        ctx.fillRect(p.x, p.y, p.size, p.size);
        ctx.globalAlpha = 1;
      }

      // Draw HUD
      ctx.fillStyle = '#39ff14';
      ctx.font = '16px monospace';
      ctx.textAlign = 'left';
      ctx.fillText(`SCORE: ${scoreRef.current}`, 10, 20);
      ctx.textAlign = 'center';
      ctx.fillText(`LEVEL ${levelRef.current}`, CANVAS_WIDTH / 2, 20);
      ctx.textAlign = 'right';
      ctx.fillText(`LIVES: ${'♥'.repeat(Math.max(0, livesRef.current))}`, CANVAS_WIDTH - 10, 20);

      // Combo display
      if (comboRef.current > 1) {
        ctx.textAlign = 'center';
        ctx.fillStyle = `rgba(255, 255, 0, ${Math.min(1, comboTimerRef.current / 1000)})`;
        ctx.font = 'bold 20px monospace';
        ctx.fillText(`COMBO x${comboRef.current}`, CANVAS_WIDTH / 2, 45);
      }

      // High score
      if (highScoreRef.current > 0) {
        ctx.textAlign = 'right';
        ctx.fillStyle = '#ffff00';
        ctx.font = '12px monospace';
        ctx.fillText(`HI: ${highScoreRef.current}`, CANVAS_WIDTH - 10, 40);
      }

      // CRT scanline effect
      ctx.fillStyle = 'rgba(0, 0, 0, 0.03)';
      for (let y = 0; y < CANVAS_HEIGHT; y += 4) {
        ctx.fillRect(0, y, CANVAS_WIDTH, 2);
      }

      // Pause overlay
      if (state === 'paused') {
        ctx.fillStyle = 'rgba(0, 0, 0, 0.7)';
        ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);
        ctx.fillStyle = '#ffff00';
        ctx.font = 'bold 48px monospace';
        ctx.textAlign = 'center';
        ctx.fillText('PAUSED', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2);
        ctx.font = '16px monospace';
        ctx.fillText('Press P to resume', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 40);
      }
    }

    if (state === 'menu') {
      // Title screen
      ctx.fillStyle = '#39ff14';
      ctx.shadowColor = '#39ff14';
      ctx.shadowBlur = 20;
      ctx.font = 'bold 48px monospace';
      ctx.textAlign = 'center';
      ctx.fillText('SPACE INVADERS', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 - 80);
      ctx.shadowBlur = 0;

      ctx.fillStyle = '#00ffff';
      ctx.font = '18px monospace';
      ctx.fillText('← → or A/D to move', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 - 20);
      ctx.fillText('SPACE to shoot', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 10);
      ctx.fillText('P to pause', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 40);

      ctx.fillStyle = '#ff6ec7';
      ctx.font = 'bold 24px monospace';
      const blink = Math.sin(timestamp / 300) > 0;
      if (blink) {
        ctx.fillText('Press START to play!', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 100);
      }

      if (highScoreRef.current > 0) {
        ctx.fillStyle = '#ffff00';
        ctx.font = '16px monospace';
        ctx.fillText(`HIGH SCORE: ${highScoreRef.current}`, CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 150);
      }
    }

    if (state === 'gameover') {
      ctx.fillStyle = 'rgba(0, 0, 0, 0.8)';
      ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);

      ctx.fillStyle = '#ff6600';
      ctx.shadowColor = '#ff6600';
      ctx.shadowBlur = 20;
      ctx.font = 'bold 48px monospace';
      ctx.textAlign = 'center';
      ctx.fillText('GAME OVER', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 - 60);
      ctx.shadowBlur = 0;

      ctx.fillStyle = '#39ff14';
      ctx.font = '24px monospace';
      ctx.fillText(`SCORE: ${scoreRef.current}`, CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2);

      if (scoreRef.current >= highScoreRef.current && scoreRef.current > 0) {
        ctx.fillStyle = '#ffff00';
        ctx.font = 'bold 20px monospace';
        ctx.fillText('★ NEW HIGH SCORE ★', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 40);
      }

      ctx.fillStyle = '#00ffff';
      ctx.font = '18px monospace';
      const blink = Math.sin(timestamp / 300) > 0;
      if (blink) {
        ctx.fillText('Press RESTART', CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 100);
      }
    }

    // Flash effect
    if (flashTimerRef.current > 0) {
      ctx.fillStyle = `rgba(255, 255, 255, ${flashTimerRef.current / 30})`;
      ctx.fillRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);
    }

    ctx.restore();

    // Update React state for observability
    if (state !== gameState) {
      setGameState(state);
    }
    setDisplayState({
      score: scoreRef.current,
      lives: livesRef.current,
      level: levelRef.current,
      gameOver: state === 'gameover',
      phase: state,
      aliensRemaining: aliensRef.current.filter((a) => a.alive).length,
      highScore: highScoreRef.current,
    });

    animFrameRef.current = requestAnimationFrame(gameLoop);
  }, [gameState, initAliens, spawnExplosion, drawSprite, drawPlayer, drawUfo]);

  // --- Start game ---
  const startGame = useCallback(() => {
    scoreRef.current = 0;
    livesRef.current = PLAYER_LIVES;
    levelRef.current = 1;
    comboRef.current = 0;
    comboTimerRef.current = 0;
    bulletsRef.current = [];
    alienBulletsRef.current = [];
    particlesRef.current = [];
    ufoRef.current.active = false;
    ufoTimerRef.current = 0;
    alienDirRef.current = 1;
    alienMoveTimerRef.current = 0;
    playerRef.current.x = CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2;
    playerRef.current.alive = true;
    initAliens();
    initStars();
    gameStateRef.current = 'playing';
    setGameState('playing');
  }, [initAliens, initStars]);

  // --- Setup game loop and event listeners ---
  useEffect(() => {
    initStars();

    const canvas = canvasRef.current;
    if (!canvas) return;

    animFrameRef.current = requestAnimationFrame(gameLoop);

    const handleKeyDown = (e: KeyboardEvent) => {
      keysRef.current.add(e.key);

      if (e.key === ' ' && gameStateRef.current === 'playing') {
        e.preventDefault();
        const bullets = bulletsRef.current;
        if (bullets.length < MAX_BULLETS) {
          const player = playerRef.current;
          bullets.push({
            x: player.x + player.width / 2 - 2,
            y: player.y - 4,
            width: 4,
            height: 12,
            color: '#39ff14',
            alive: true,
            vy: -BULLET_SPEED,
            isPlayer: true,
          });
        }
      }

      if (e.key === 'p' || e.key === 'P') {
        if (gameStateRef.current === 'playing') {
          gameStateRef.current = 'paused';
          setGameState('paused');
        } else if (gameStateRef.current === 'paused') {
          gameStateRef.current = 'playing';
          setGameState('playing');
        }
      }

      // R key for restart during game
      if (e.key === 'r' || e.key === 'R') {
        if (gameStateRef.current === 'gameover') {
          startGame();
        }
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      keysRef.current.delete(e.key);
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);

    return () => {
      cancelAnimationFrame(animFrameRef.current);
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
    };
  }, [gameLoop, initStars, startGame]);

  return (
    <div
      className="flex flex-col items-center gap-4 p-4"
      data-anvil-state={JSON.stringify({
        score: displayState.score,
        lives: displayState.lives,
        level: displayState.level,
        gameOver: displayState.gameOver,
        phase: displayState.phase,
        aliensRemaining: displayState.aliensRemaining,
        highScore: displayState.highScore,
      })}
    >
      {/* Scoreboard */}
      <div className="flex justify-between w-full max-w-[640px] text-neon-green font-mono text-sm">
        <span>SCORE: {displayState.score}</span>
        <span>LEVEL {displayState.level}</span>
        <span>LIVES: {'♥'.repeat(Math.max(0, displayState.lives))}</span>
      </div>

      {/* Game Canvas */}
      <canvas
        ref={canvasRef}
        width={CANVAS_WIDTH}
        height={CANVAS_HEIGHT}
        className="border-2 border-neon-green box-glow crt-scanline"
        style={{ imageRendering: 'pixelated', maxWidth: '100%' }}
      />

      {/* Controls */}
      <div className="flex gap-4">
        <button
          data-anvil-action="primary"
          onClick={startGame}
          className="px-6 py-3 bg-space-mid border-2 border-neon-pink text-neon-pink text-lg font-bold hover:bg-neon-pink hover:text-space-dark transition-all duration-200 box-glow"
        >
          {displayState.phase === 'gameover' ? 'RESTART' : displayState.phase === 'playing' ? 'RESTART' : 'START GAME'}
        </button>
        {displayState.phase === 'playing' && (
          <button
            onClick={() => {
              gameStateRef.current = 'paused';
              setGameState('paused');
            }}
            className="px-6 py-3 bg-space-mid border-2 border-neon-yellow text-neon-yellow text-lg font-bold hover:bg-neon-yellow hover:text-space-dark transition-all duration-200"
          >
            PAUSE
          </button>
        )}
      </div>

      {/* Instructions */}
      <div className="text-neon-cyan text-sm font-mono text-center">
        <p>← → or A/D to move | SPACE to shoot | P to pause | R to restart</p>
      </div>
    </div>
  );
}

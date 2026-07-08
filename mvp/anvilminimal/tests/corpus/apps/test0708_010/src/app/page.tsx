'use client';

import { useEffect, useRef, useState, useCallback } from 'react';

// ─── Constants ───────────────────────────────────────────────────────
const CANVAS_W = 800;
const CANVAS_H = 600;
const PLAYER_W = 40;
const PLAYER_H = 30;
const PLAYER_SPEED = 5;
const BULLET_SPEED = 7;
const ENEMY_BULLET_SPEED = 3;
const ENEMY_COLS = 8;
const ENEMY_ROWS = 4;
const ENEMY_W = 36;
const ENEMY_H = 28;
const ENEMY_PADDING = 12;
const ENEMY_START_Y = 60;
const ENEMY_DROP_PX = 20;
const PLAYER_Y = CANVAS_H - 60;
const PARTICLE_LIFE = 30;

type Vec2 = { x: number; y: number };

interface Enemy {
  x: number;
  y: number;
  alive: boolean;
  type: number; // 0, 1, 2 for different sprites
  frame: number;
}

interface Bullet {
  x: number;
  y: number;
  dy: number;
  fromEnemy: boolean;
}

interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  color: string;
}

interface Star {
  x: number;
  y: number;
  size: number;
  speed: number;
  opacity: number;
}

type GameState = 'menu' | 'playing' | 'gameover' | 'victory';

// ─── Colors ──────────────────────────────────────────────────────────
const COLORS = {
  player: '#39ff14',
  playerBullet: '#00d4ff',
  enemyBullet: '#ff073a',
  enemy: ['#ff6ec7', '#ffd700', '#39ff14', '#00d4ff'],
  explosion: ['#ff6ec7', '#ffd700', '#ff073a', '#ffaa00'],
  star: ['#ffffff', '#aaddff', '#ffddaa'],
};

// ─── Helper ──────────────────────────────────────────────────────────
function rectsOverlap(
  ax: number, ay: number, aw: number, ah: number,
  bx: number, by: number, bw: number, bh: number
): boolean {
  return ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by;
}

function createStars(count: number): Star[] {
  return Array.from({ length: count }, () => ({
    x: Math.random() * CANVAS_W,
    y: Math.random() * CANVAS_H,
    size: Math.random() * 2 + 0.5,
    speed: Math.random() * 0.5 + 0.1,
    opacity: Math.random() * 0.8 + 0.2,
  }));
}

function createEnemies(): Enemy[] {
  const enemies: Enemy[] = [];
  const totalW = ENEMY_COLS * (ENEMY_W + ENEMY_PADDING) - ENEMY_PADDING;
  const startX = (CANVAS_W - totalW) / 2;
  for (let row = 0; row < ENEMY_ROWS; row++) {
    for (let col = 0; col < ENEMY_COLS; col++) {
      enemies.push({
        x: startX + col * (ENEMY_W + ENEMY_PADDING),
        y: ENEMY_START_Y + row * (ENEMY_H + ENEMY_PADDING),
        alive: true,
        type: row % 3,
        frame: 0,
      });
    }
  }
  return enemies;
}

// ─── Main Component ──────────────────────────────────────────────────
export default function Home() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const animRef = useRef<number>(0);
  const keysRef = useRef<Set<string>>(new Set());
  const playerRef = useRef<Vec2>({ x: CANVAS_W / 2 - PLAYER_W / 2, y: PLAYER_Y });
  const bulletsRef = useRef<Bullet[]>([]);
  const enemyBulletsRef = useRef<Bullet[]>([]);
  const enemiesRef = useRef<Enemy[]>([]);
  const particlesRef = useRef<Particle[]>([]);
  const starsRef = useRef<Star[]>([]);
  const dirRef = useRef(1);
  const enemySpeedRef = useRef(1.2);
  const enemyFrameRef = useRef(0);
  const lastShotRef = useRef(0);
  const lastEnemyShotRef = useRef(0);
  const scoreRef = useRef(0);
  const livesRef = useRef(3);
  const gameStateRef = useRef<GameState>('menu');
  const shakeRef = useRef(0);
  const levelRef = useRef(1);

  const [score, setScore] = useState(0);
  const [lives, setLives] = useState(3);
  const [gameState, setGameState] = useState<GameState>('menu');
  const [level, setLevel] = useState(1);
  const [highScore, setHighScore] = useState(0);

  // ── Initialize / Reset ────────────────────────────────────────────
  const initGame = useCallback((lvl: number = 1) => {
    playerRef.current = { x: CANVAS_W / 2 - PLAYER_W / 2, y: PLAYER_Y };
    bulletsRef.current = [];
    enemyBulletsRef.current = [];
    enemiesRef.current = createEnemies();
    particlesRef.current = [];
    starsRef.current = createStars(100);
    dirRef.current = 1;
    enemySpeedRef.current = 1.2 + lvl * 0.3;
    enemyFrameRef.current = 0;
    scoreRef.current = 0;
    livesRef.current = 3;
    shakeRef.current = 0;
    levelRef.current = lvl;
    setScore(0);
    setLives(3);
    setLevel(lvl);
  }, []);

  const startGame = useCallback(() => {
    initGame(1);
    gameStateRef.current = 'playing';
    setGameState('playing');
  }, [initGame]);

  const nextLevel = useCallback(() => {
    const nextLvl = levelRef.current + 1;
    initGame(nextLvl);
    gameStateRef.current = 'playing';
    setGameState('playing');
  }, [initGame]);

  // ── Explosion Particles ───────────────────────────────────────────
  const spawnExplosion = useCallback((x: number, y: number) => {
    const count = 15 + Math.floor(Math.random() * 10);
    for (let i = 0; i < count; i++) {
      const angle = Math.random() * Math.PI * 2;
      const speed = Math.random() * 3 + 1;
      particlesRef.current.push({
        x,
        y,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed,
        life: PARTICLE_LIFE + Math.floor(Math.random() * 10),
        color: COLORS.explosion[Math.floor(Math.random() * COLORS.explosion.length)],
      });
    }
  }, []);

  // ── Drawing Helpers ───────────────────────────────────────────────
  const drawPlayer = useCallback((ctx: CanvasRenderingContext2D, px: number, py: number) => {
    ctx.save();
    ctx.shadowBlur = 15;
    ctx.shadowColor = COLORS.player;
    ctx.fillStyle = COLORS.player;
    // Ship body
    ctx.beginPath();
    ctx.moveTo(px + PLAYER_W / 2, py);
    ctx.lineTo(px + PLAYER_W, py + PLAYER_H);
    ctx.lineTo(px + PLAYER_W - 6, py + PLAYER_H);
    ctx.lineTo(px + PLAYER_W / 2, py + PLAYER_H - 8);
    ctx.lineTo(px + 6, py + PLAYER_H);
    ctx.lineTo(px, py + PLAYER_H);
    ctx.closePath();
    ctx.fill();
    // Cockpit
    ctx.fillStyle = '#ffffff';
    ctx.beginPath();
    ctx.arc(px + PLAYER_W / 2, py + PLAYER_H * 0.5, 4, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }, []);

  const drawEnemy = useCallback((ctx: CanvasRenderingContext2D, e: Enemy) => {
    if (!e.alive) return;
    ctx.save();
    ctx.shadowBlur = 8;
    ctx.shadowColor = COLORS.enemy[e.type];
    ctx.fillStyle = COLORS.enemy[e.type];
    const cx = e.x + ENEMY_W / 2;
    const cy = e.y + ENEMY_H / 2;
    const w = ENEMY_W / 2;
    const h = ENEMY_H / 2;
    // Simple alien shape
    ctx.beginPath();
    ctx.arc(cx, cy - h * 0.2, w * 0.6, Math.PI, 0);
    ctx.lineTo(cx + w * 0.6, cy + h * 0.3);
    // Legs
    for (let i = -1; i <= 1; i++) {
      ctx.lineTo(cx + i * w * 0.4, cy + h);
      ctx.lineTo(cx + i * w * 0.4 + (e.frame % 2 === 0 ? 4 : -4), cy + h * 0.5);
    }
    ctx.lineTo(cx - w * 0.6, cy + h * 0.3);
    ctx.closePath();
    ctx.fill();
    // Eyes
    ctx.fillStyle = '#000';
    ctx.beginPath();
    ctx.arc(cx - w * 0.25, cy - h * 0.15, 3, 0, Math.PI * 2);
    ctx.arc(cx + w * 0.25, cy - h * 0.15, 3, 0, Math.PI * 2);
    ctx.fill();
    ctx.restore();
  }, []);

  // ── Main Game Loop ────────────────────────────────────────────────
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    starsRef.current = createStars(120);

    let lastTime = 0;
    let enemyMoveTimer = 0;

    const gameLoop = (time: number) => {
      const dt = Math.min((time - lastTime) / 16.67, 3); // normalize to ~60fps
      lastTime = time;

      const gs = gameStateRef.current;

      // ── Shake decay ───────────────────────────────────────────────
      if (shakeRef.current > 0) shakeRef.current = Math.max(0, shakeRef.current - 0.5 * dt);

      // ── Clear + background ────────────────────────────────────────
      ctx.save();
      if (shakeRef.current > 0) {
        ctx.translate(
          (Math.random() - 0.5) * shakeRef.current * 2,
          (Math.random() - 0.5) * shakeRef.current * 2
        );
      }
      ctx.fillStyle = '#000000';
      ctx.fillRect(0, 0, CANVAS_W, CANVAS_H);

      // Stars
      starsRef.current.forEach((s) => {
        s.y += s.speed * dt;
        if (s.y > CANVAS_H) { s.y = 0; s.x = Math.random() * CANVAS_W; }
        ctx.globalAlpha = s.opacity * (0.5 + 0.5 * Math.sin(time * 0.003 + s.x));
        ctx.fillStyle = COLORS.star[Math.floor(s.x * 7) % 3 === 0 ? 0 : Math.floor(s.x * 13) % 3 === 0 ? 1 : 2];
        ctx.fillRect(s.x, s.y, s.size, s.size);
      });
      ctx.globalAlpha = 1;

      if (gs === 'menu') {
        // ── Menu Screen ─────────────────────────────────────────────
        ctx.textAlign = 'center';
        ctx.shadowBlur = 20;
        ctx.shadowColor = '#39ff14';
        ctx.fillStyle = '#39ff14';
        ctx.font = 'bold 56px monospace';
        ctx.fillText('SPACE INVADERS', CANVAS_W / 2, CANVAS_H / 2 - 60);
        ctx.font = '18px monospace';
        ctx.fillStyle = '#aaddff';
        ctx.shadowBlur = 10;
        ctx.fillText('← → to move  |  SPACE to shoot', CANVAS_W / 2, CANVAS_H / 2 + 10);
        ctx.fillText('Survive the alien invasion!', CANVAS_W / 2, CANVAS_H / 2 + 40);
        ctx.fillStyle = '#ffd700';
        ctx.font = 'bold 22px monospace';
        ctx.fillText('Press ENTER to Start', CANVAS_W / 2, CANVAS_H / 2 + 100);
        ctx.restore();
        animRef.current = requestAnimationFrame(gameLoop);
        return;
      }

      // ── Playing / GameOver / Victory ──────────────────────────────
      if (gs === 'gameover') {
        // Draw remaining enemies frozen
        enemiesRef.current.forEach((e) => drawEnemy(ctx, e));
        ctx.textAlign = 'center';
        ctx.shadowBlur = 20;
        ctx.shadowColor = '#ff073a';
        ctx.fillStyle = '#ff073a';
        ctx.font = 'bold 52px monospace';
        ctx.fillText('GAME OVER', CANVAS_W / 2, CANVAS_H / 2 - 30);
        ctx.font = '18px monospace';
        ctx.fillStyle = '#aaddff';
        ctx.shadowBlur = 10;
        ctx.fillText(`Final Score: ${scoreRef.current}`, CANVAS_W / 2, CANVAS_H / 2 + 20);
        ctx.fillText('Press ENTER to Restart', CANVAS_W / 2, CANVAS_H / 2 + 60);
        ctx.restore();
        animRef.current = requestAnimationFrame(gameLoop);
        return;
      }

      if (gs === 'victory') {
        ctx.textAlign = 'center';
        ctx.shadowBlur = 20;
        ctx.shadowColor = '#ffd700';
        ctx.fillStyle = '#ffd700';
        ctx.font = 'bold 48px monospace';
        ctx.fillText('VICTORY!', CANVAS_W / 2, CANVAS_H / 2 - 30);
        ctx.font = '18px monospace';
        ctx.fillStyle = '#aaddff';
        ctx.shadowBlur = 10;
        ctx.fillText(`Score: ${scoreRef.current}`, CANVAS_W / 2, CANVAS_H / 2 + 20);
        ctx.fillText(`Level: ${levelRef.current}`, CANVAS_W / 2, CANVAS_H / 2 + 50);
        ctx.restore();
        animRef.current = requestAnimationFrame(gameLoop);
        return;
      }

      // ── PLAYING UPDATE ────────────────────────────────────────────
      const keys = keysRef.current;
      const player = playerRef.current;

      // Player movement
      if (keys.has('ArrowLeft') || keys.has('a')) {
        player.x = Math.max(0, player.x - PLAYER_SPEED * dt);
      }
      if (keys.has('ArrowRight') || keys.has('d')) {
        player.x = Math.min(CANVAS_W - PLAYER_W, player.x + PLAYER_SPEED * dt);
      }

      // Shooting
      if (keys.has(' ') && time - lastShotRef.current > 300) {
        bulletsRef.current.push({ x: player.x + PLAYER_W / 2 - 2, y: PLAYER_Y - 10, dy: -BULLET_SPEED, fromEnemy: false });
        lastShotRef.current = time;
      }

      // Update player bullets
      bulletsRef.current = bulletsRef.current.filter((b) => {
        b.y += b.dy * dt;
        return b.y > -10;
      });

      // Enemy movement
      enemyMoveTimer += dt;
      const aliveEnemies = enemiesRef.current.filter((e) => e.alive);
      if (aliveEnemies.length > 0 && enemyMoveTimer > Math.max(5, 20 - levelRef.current * 2)) {
        enemyMoveTimer = 0;
        enemyFrameRef.current = (enemyFrameRef.current + 1) % 2;

        let shouldDrop = false;
        const speed = enemySpeedRef.current * dirRef.current;
        for (const e of aliveEnemies) {
          if ((dirRef.current === 1 && e.x + ENEMY_W + speed > CANVAS_W - 10) ||
              (dirRef.current === -1 && e.x + speed < 10)) {
            shouldDrop = true;
            break;
          }
        }
        if (shouldDrop) {
          dirRef.current *= -1;
          for (const e of aliveEnemies) {
            e.y += ENEMY_DROP_PX;
          }
        } else {
          for (const e of aliveEnemies) {
            e.x += speed;
          }
        }
      }

      // Enemy shooting
      if (time - lastEnemyShotRef.current > Math.max(400, 1200 - levelRef.current * 100)) {
        lastEnemyShotRef.current = time;
        const shooters = aliveEnemies.filter((e) => e.y + ENEMY_H >= CANVAS_H - 100);
        if (shooters.length > 0) {
          const shooter = shooters[Math.floor(Math.random() * shooters.length)];
          enemyBulletsRef.current.push({
            x: shooter.x + ENEMY_W / 2 - 2,
            y: shooter.y + ENEMY_H,
            dy: ENEMY_BULLET_SPEED,
            fromEnemy: true,
          });
        }
      }

      // Update enemy bullets
      enemyBulletsRef.current = enemyBulletsRef.current.filter((b) => {
        b.y += b.dy * dt;
        return b.y < CANVAS_H + 10;
      });

      // Bullet-enemy collision
      const bulletsToRemove: number[] = [];
      const enemiesToRemove: Enemy[] = [];
      for (let bi = 0; bi < bulletsRef.current.length; bi++) {
        const b = bulletsRef.current[bi];
        for (let ei = 0; ei < enemiesRef.current.length; ei++) {
          const e = enemiesRef.current[ei];
          if (!e.alive) continue;
          if (rectsOverlap(b.x, b.y, 4, 12, e.x, e.y, ENEMY_W, ENEMY_H)) {
            e.alive = false;
            bulletsToRemove.push(bi);
            spawnExplosion(e.x + ENEMY_W / 2, e.y + ENEMY_H / 2);
            scoreRef.current += (3 - e.type + 1) * 10;
            shakeRef.current = 5;
            break;
          }
        }
      }
      bulletsToRemove.reverse().forEach((i) => bulletsRef.current.splice(i, 1));
      enemiesRef.current.forEach((e) => { if (!e.alive) { spawnExplosion(e.x + ENEMY_W / 2, e.y + ENEMY_H / 2); } });
      enemiesRef.current = enemiesRef.current.filter((e) => e.alive);

      // Enemy bullet-player collision
      let hitPlayer = false;
      for (const b of enemyBulletsRef.current) {
        if (rectsOverlap(b.x, b.y, 4, 12, player.x, player.y, PLAYER_W, PLAYER_H)) {
          hitPlayer = true;
          break;
        }
      }
      if (hitPlayer) {
        spawnExplosion(player.x + PLAYER_W / 2, player.y + PLAYER_H / 2);
        shakeRef.current = 15;
        enemyBulletsRef.current = [];
        livesRef.current -= 1;
        if (livesRef.current <= 0) {
          gameStateRef.current = 'gameover';
          setGameState('gameover');
          setScore(scoreRef.current);
          if (scoreRef.current > highScore) setHighScore(scoreRef.current);
        }
      }

      // Enemy reaches player
      for (const e of aliveEnemies) {
        if (e.y + ENEMY_H >= player.y) {
          gameStateRef.current = 'gameover';
          setGameState('gameover');
          setScore(scoreRef.current);
          shakeRef.current = 20;
          break;
        }
      }

      // Victory check
      if (aliveEnemies.length === 0 && gameStateRef.current === 'playing') {
        gameStateRef.current = 'victory';
        setGameState('victory');
        setScore(scoreRef.current);
      }

      // Update particles
      particlesRef.current = particlesRef.current.filter((p) => {
        p.x += p.vx * dt;
        p.y += p.vy * dt;
        p.life -= dt;
        return p.life > 0;
      });

      // ── DRAW ──────────────────────────────────────────────────────
      // Player
      if (gameStateRef.current !== 'gameover') {
        drawPlayer(ctx, player.x, player.y);
      }

      // Bullets
      ctx.shadowBlur = 10;
      for (const b of bulletsRef.current) {
        ctx.shadowColor = COLORS.playerBullet;
        ctx.fillStyle = COLORS.playerBullet;
        ctx.fillRect(b.x, b.y, 3, 12);
      }
      for (const b of enemyBulletsRef.current) {
        ctx.shadowColor = COLORS.enemyBullet;
        ctx.fillStyle = COLORS.enemyBullet;
        ctx.fillRect(b.x, b.y, 3, 12);
      }

      // Enemies
      for (const e of enemiesRef.current) {
        drawEnemy(ctx, e);
      }

      // Particles
      for (const p of particlesRef.current) {
        ctx.globalAlpha = p.life / PARTICLE_LIFE;
        ctx.fillStyle = p.color;
        ctx.shadowBlur = 5;
        ctx.shadowColor = p.color;
        ctx.fillRect(p.x - 1, p.y - 1, 3, 3);
      }
      ctx.globalAlpha = 1;

      // HUD
      ctx.shadowBlur = 0;
      ctx.textAlign = 'left';
      ctx.fillStyle = '#39ff14';
      ctx.font = 'bold 18px monospace';
      ctx.fillText(`SCORE: ${scoreRef.current}`, 15, 25);
      ctx.textAlign = 'right';
      ctx.fillText(`LEVEL ${levelRef.current}`, CANVAS_W - 15, 25);
      ctx.textAlign = 'center';
      // Lives as little ship icons
      for (let i = 0; i < livesRef.current; i++) {
        const lx = CANVAS_W / 2 - 30 + i * 25;
        ctx.fillStyle = '#39ff14';
        ctx.fillRect(lx, CANVAS_H - 20, 10, 6);
        ctx.fillRect(lx + 3, CANVAS_H - 24, 4, 4);
      }

      // Update React state periodically
      setScore(scoreRef.current);
      setLives(livesRef.current);

      ctx.restore();
      animRef.current = requestAnimationFrame(gameLoop);
    };

    animRef.current = requestAnimationFrame(gameLoop);
    return () => cancelAnimationFrame(animRef.current);
  }, [drawPlayer, drawEnemy, spawnExplosion, highScore]);

  // ── Keyboard Handlers ─────────────────────────────────────────────
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      keysRef.current.add(e.key);

      if (e.key === 'Enter') {
        const gs = gameStateRef.current;
        if (gs === 'menu') startGame();
        else if (gs === 'gameover') startGame();
        else if (gs === 'victory') nextLevel();
      }
      e.preventDefault();
    };
    const onKeyUp = (e: KeyboardEvent) => {
      keysRef.current.delete(e.key);
    };
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
    };
  }, [startGame, nextLevel]);

  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-black overflow-hidden">
      <canvas
        ref={canvasRef}
        width={CANVAS_W}
        height={CANVAS_H}
        className="border border-neon-green/30 rounded-lg shadow-[0_0_30px_rgba(57,255,20,0.3)]"
        data-anvil-action="primary"
        data-anvil-state={JSON.stringify({ score, lives, gameState, level })}
      />
      <div className="mt-4 text-center">
        <p className="text-neon-green/60 text-sm font-mono">
          ← → Move &nbsp;|&nbsp; SPACE Shoot &nbsp;|&nbsp; ENTER Start/Restart
        </p>
      </div>
    </div>
  );
}

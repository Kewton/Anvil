"use client";

import { useRef, useEffect, useCallback, useState } from "react";

// ─── Types ────────────────────────────────────────────────────────────────────

interface Vec2 {
  x: number;
  y: number;
}

interface Bullet extends Vec2 {
  id: number;
  speed: number;
  isPlayer: boolean;
}

interface Invader extends Vec2 {
  id: number;
  type: "squid" | "crab" | "octopus";
  alive: boolean;
  frame: number;
}

interface Particle extends Vec2 {
  id: number;
  vx: number;
  vy: number;
  life: number;
  maxLife: number;
  color: string;
  size: number;
}

interface Star extends Vec2 {
  speed: number;
  brightness: number;
  size: number;
}

type GameState = "menu" | "playing" | "paused" | "gameover" | "victory";

interface GameData {
  state: GameState;
  score: number;
  highScore: number;
  lives: number;
  level: number;
  combo: number;
  maxCombo: number;
}

// ─── Constants ────────────────────────────────────────────────────────────────

const CANVAS_W = 800;
const CANVAS_H = 640;
const PLAYER_W = 40;
const PLAYER_H = 24;
const PLAYER_SPEED = 6;
const BULLET_SPEED = 9;
const ENEMY_BULLET_SPEED = 4.5;
const INVADER_COLS = 8;
const INVADER_ROWS = 4;
const INVADER_W = 32;
const INVADER_H = 24;
const INVADER_PAD_X = 12;
const INVADER_PAD_Y = 10;
const INVADER_START_Y = 60;
const PARTICLE_POOL_SIZE = 200;

const COLORS: Record<string, string> = {
  player: "#00f5ff",
  playerGlow: "rgba(0,245,255,0.3)",
  bulletPlayer: "#00ffcc",
  bulletEnemy: "#ff006e",
  squid: "#bf00ff",
  crab: "#ffaa00",
  octopus: "#00f5ff",
  particle1: "#00f5ff",
  particle2: "#bf00ff",
  particle3: "#ff006e",
  particle4: "#ffaa00",
  white: "#ffffff",
};

// ─── Helpers ──────────────────────────────────────────────────────────────────

let nextId = 1;
const uid = () => ++nextId;

function rectOverlap(
  ax: number, ay: number, aw: number, ah: number,
  bx: number, by: number, bw: number, bh: number
): boolean {
  return ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by;
}

function spawnExplosion(
  x: number, y: number, color: string, count: number
): Particle[] {
  const particles: Particle[] = [];
  for (let i = 0; i < count; i++) {
    const angle = Math.random() * Math.PI * 2;
    const speed = 1.5 + Math.random() * 4;
    particles.push({
      id: uid(),
      x, y,
      vx: Math.cos(angle) * speed,
      vy: Math.sin(angle) * speed,
      life: 30 + Math.random() * 25,
      maxLife: 55,
      color,
      size: 2 + Math.random() * 4,
    });
  }
  return particles;
}

function initStars(count: number): Star[] {
  const stars: Star[] = [];
  for (let i = 0; i < count; i++) {
    stars.push({
      x: Math.random() * CANVAS_W,
      y: Math.random() * CANVAS_H,
      speed: 0.3 + Math.random() * 1.5,
      brightness: 0.3 + Math.random() * 0.7,
      size: 0.5 + Math.random() * 2,
    });
  }
  return stars;
}

function initInvaders(level: number): Invader[] {
  const invaders: Invader[] = [];
  const types: Array<"squid" | "crab" | "octopus"> = ["octopus", "crab", "crab", "squid"];
  for (let row = 0; row < INVADER_ROWS; row++) {
    for (let col = 0; col < INVADER_COLS; col++) {
      invaders.push({
        id: uid(),
        x: 80 + col * (INVADER_W + INVADER_PAD_X),
        y: INVADER_START_Y + row * (INVADER_H + INVADER_PAD_Y),
        type: types[row] || "squid",
        alive: true,
        frame: 0,
      });
    }
  }
  return invaders;
}

// ─── Hook ─────────────────────────────────────────────────────────────────────

export function useGame() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const animFrameRef = useRef<number>(0);
  const keysRef = useRef<Set<string>>(new Set());
  const lastShotRef = useRef(0);
  const invaderDirRef = useRef(1);
  const invaderMoveTimerRef = useRef(0);
  const invaderShootTimerRef = useRef(0);
  const frameCountRef = useRef(0);

  // Game state (React state for data-anvil observability)
  const [gameData, setGameData] = useState<GameData>({
    state: "menu",
    score: 0,
    highScore: parseInt(localStorage.getItem("neon-invaders-high") || "0", 10),
    lives: 3,
    level: 1,
    combo: 0,
    maxCombo: 0,
  });

  // Mutable game objects (ref-based for perf)
  const playerRef = useRef<Vec2>({ x: CANVAS_W / 2 - PLAYER_W / 2, y: CANVAS_H - 50 });
  const bulletsRef = useRef<Bullet[]>([]);
  const invadersRef = useRef<Invader[]>(initInvaders(1));
  const particlesRef = useRef<Particle[]>([]);
  const starsRef = useRef<Star[]>(initStars(120));
  const shakeRef = useRef(0);

  // Sync mutable state into React for data-anvil-state serialization
  const syncState = useCallback(() => {
    setGameData((prev) => ({ ...prev }));
  }, []);

  // ─── Input ──────────────────────────────────────────────────────────────

  useEffect(() => {
    const down = (e: KeyboardEvent) => {
      keysRef.current.add(e.key);
      if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", " "].includes(e.key)) {
        e.preventDefault();
      }
      // Restart on Enter when game over / menu
      if ((e.key === "Enter" || e.key === " ") && gameData.state !== "playing") {
        startGame();
      }
    };
    const up = (e: KeyboardEvent) => keysRef.current.delete(e.key);
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
    };
  }, [gameData.state]);

  // ─── Game start / restart ───────────────────────────────────────────────

  const startGame = useCallback(() => {
    playerRef.current = { x: CANVAS_W / 2 - PLAYER_W / 2, y: CANVAS_H - 50 };
    bulletsRef.current = [];
    invadersRef.current = initInvaders(gameData.level);
    particlesRef.current = [];
    invaderDirRef.current = 1;
    invaderMoveTimerRef.current = 0;
    invaderShootTimerRef.current = 0;
    frameCountRef.current = 0;
    setGameData((prev) => ({
      ...prev,
      state: "playing",
      score: prev.state === "gameover" ? 0 : prev.score,
      lives: prev.state === "gameover" ? 3 : prev.lives,
      combo: 0,
    }));
  }, [gameData.level]);

  // ─── Game loop ──────────────────────────────────────────────────────────

  const update = useCallback(() => {
    if (gameData.state !== "playing") return;

    frameCountRef.current++;
    const fc = frameCountRef.current;
    const keys = keysRef.current;
    const player = playerRef.current;
    const bullets = bulletsRef.current;
    const invaders = invadersRef.current;
    const particles = particlesRef.current;

    // ── Player movement ──
    if (keys.has("ArrowLeft") || keys.has("a") || keys.has("A")) {
      player.x = Math.max(0, player.x - PLAYER_SPEED);
    }
    if (keys.has("ArrowRight") || keys.has("d") || keys.has("D")) {
      player.x = Math.min(CANVAS_W - PLAYER_W, player.x + PLAYER_SPEED);
    }

    // ── Player shooting (rate-limited) ──
    const now = performance.now();
    if ((keys.has(" ") || keys.has("ArrowUp") || keys.has("w") || keys.has("W")) && now - lastShotRef.current > 200) {
      lastShotRef.current = now;
      bullets.push({
        id: uid(),
        x: player.x + PLAYER_W / 2 - 2,
        y: player.y,
        speed: -BULLET_SPEED,
        isPlayer: true,
      });
    }

    // ── Update bullets ──
    for (let i = bullets.length - 1; i >= 0; i--) {
      const b = bullets[i];
      b.y += b.speed;
      if (b.y < -10 || b.y > CANVAS_H + 10) {
        bullets.splice(i, 1);
        if (!b.isPlayer) {
          setGameData((prev) => ({ ...prev, combo: 0 }));
        }
      }
    }

    // ── Invader movement ──
    const aliveInvaders = invaders.filter((inv) => inv.alive);
    if (aliveInvaders.length === 0) {
      // Level cleared — next level
      setGameData((prev) => ({ ...prev, level: prev.level + 1 }));
      startGame();
      return;
    }

    const moveInterval = Math.max(4, 30 - gameData.level * 2);
    invaderMoveTimerRef.current++;
    if (invaderMoveTimerRef.current >= moveInterval) {
      invaderMoveTimerRef.current = 0;
      const dir = invaderDirRef.current;

      // Check edges
      let needDrop = false;
      for (const inv of aliveInvaders) {
        if ((dir > 0 && inv.x + INVADER_W / 2 + 20 >= CANVAS_W - 20) ||
            (dir < 0 && inv.x - INVADER_W / 2 - 20 <= 20)) {
          needDrop = true;
          break;
        }
      }

      if (needDrop) {
        invaderDirRef.current *= -1;
        for (const inv of aliveInvaders) inv.y += 16;
      } else {
        const step = 8 + gameData.level;
        for (const inv of aliveInvaders) inv.x += dir * step;
      }

      // Animate frames
      for (const inv of aliveInvaders) inv.frame ^= 1;
    }

    // ── Invader shooting ──
    invaderShootTimerRef.current++;
    const shootInterval = Math.max(20, 60 - gameData.level * 5);
    if (invaderShootTimerRef.current >= shootInterval && aliveInvaders.length > 0) {
      invaderShootTimerRef.current = 0;
      // Pick a random bottom-row invader
      const bottom: Invader[] = [];
      for (const inv of aliveInvaders) {
        if (!bottom.length || inv.y >= bottom[0].y) bottom.push(inv);
      }
      const shooter = bottom[Math.floor(Math.random() * bottom.length)];
      bullets.push({
        id: uid(),
        x: shooter.x,
        y: shooter.y + INVADER_H / 2,
        speed: ENEMY_BULLET_SPEED,
        isPlayer: false,
      });
    }

    // ── Collision detection ──
    for (let i = bullets.length - 1; i >= 0; i--) {
      const b = bullets[i];

      if (b.isPlayer) {
        // Check vs invaders
        let hit = false;
        for (const inv of aliveInvaders) {
          if (!inv.alive) continue;
          const ix = inv.x - INVADER_W / 2;
          const iy = inv.y - INVADER_H / 2;
          if (rectOverlap(b.x, b.y, 4, 10, ix, iy, INVADER_W, INVADER_H)) {
            inv.alive = false;
            bullets.splice(i, 1);
            hit = true;
            const pts = inv.type === "squid" ? 30 : inv.type === "crab" ? 20 : 10;
            setGameData((prev) => {
              const newCombo = prev.combo + 1;
              const multiplier = Math.min(newCombo, 5);
              return {
                ...prev,
                score: prev.score + pts * multiplier,
                combo: newCombo,
                maxCombo: Math.max(prev.maxCombo, newCombo),
              };
            });
            particles.push(...spawnExplosion(inv.x, inv.y, COLORS[inv.type], 12));
            shakeRef.current = 4;
            break;
          }
        }
      } else {
        // Enemy bullet vs player
        if (rectOverlap(b.x, b.y, 4, 10, player.x, player.y, PLAYER_W, PLAYER_H)) {
          bullets.splice(i, 1);
          particles.push(...spawnExplosion(player.x + PLAYER_W / 2, player.y + PLAYER_H / 2, COLORS.player, 20));
          shakeRef.current = 8;
          setGameData((prev) => {
            const newLives = prev.lives - 1;
            if (newLives <= 0) {
              const hs = Math.max(prev.score, prev.highScore);
              localStorage.setItem("neon-invaders-high", String(hs));
              return { ...prev, lives: 0, state: "gameover" as GameState, highScore: hs };
            }
            return { ...prev, lives: newLives, combo: 0 };
          });
        }
      }
    }

    // ── Invaders reach bottom ──
    for (const inv of aliveInvaders) {
      if (inv.y + INVADER_H / 2 >= player.y) {
        setGameData((prev) => {
          const hs = Math.max(prev.score, prev.highScore);
          localStorage.setItem("neon-invaders-high", String(hs));
          return { ...prev, state: "gameover" as GameState, highScore: hs };
        });
        return;
      }
    }

    // ── Update particles ──
    for (let i = particles.length - 1; i >= 0; i--) {
      const p = particles[i];
      p.x += p.vx;
      p.y += p.vy;
      p.vy += 0.08; // gravity
      p.life--;
      if (p.life <= 0) particles.splice(i, 1);
    }

    // ── Update stars ──
    for (const s of starsRef.current) {
      s.y += s.speed;
      if (s.y > CANVAS_H) { s.y = -5; s.x = Math.random() * CANVAS_W; }
    }

    // ── Screen shake decay ──
    if (shakeRef.current > 0) shakeRef.current *= 0.85;
    if (shakeRef.current < 0.3) shakeRef.current = 0;

    syncState();
  }, [gameData.state, gameData.level, syncState, startGame]);

  // ─── Render ─────────────────────────────────────────────────────────────

  const render = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const shakeX = (Math.random() - 0.5) * shakeRef.current * 2;
    const shakeY = (Math.random() - 0.5) * shakeRef.current * 2;
    ctx.save();
    ctx.translate(shakeX, shakeY);

    // Background
    ctx.fillStyle = "#0a0e27";
    ctx.fillRect(-10, -10, CANVAS_W + 20, CANVAS_H + 20);

    // Stars
    for (const s of starsRef.current) {
      ctx.globalAlpha = s.brightness * (0.5 + 0.5 * Math.sin(frameCountRef.current * 0.02 + s.x));
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(s.x, s.y, s.size, s.size);
    }
    ctx.globalAlpha = 1;

    // Invaders
    for (const inv of invadersRef.current) {
      if (!inv.alive) continue;
      const ix = inv.x - INVADER_W / 2;
      const iy = inv.y - INVADER_H / 2;
      const color = COLORS[inv.type];

      // Glow
      ctx.shadowColor = color;
      ctx.shadowBlur = 12;
      ctx.fillStyle = color;

      // Pixel-art style shapes per type
      if (inv.type === "squid") {
        drawSquid(ctx, ix, iy);
      } else if (inv.type === "crab") {
        drawCrab(ctx, ix, iy, inv.frame);
      } else {
        drawOctopus(ctx, ix, iy, inv.frame);
      }
      ctx.shadowBlur = 0;

      // Eyes
      ctx.fillStyle = "#0a0e27";
      ctx.fillRect(inv.x - 6, iy + 6, 3, 3);
      ctx.fillRect(inv.x + 3, iy + 6, 3, 3);
    }

    // Player
    const p = playerRef.current;
    if (gameData.state !== "gameover") {
      ctx.shadowColor = COLORS.player;
      ctx.shadowBlur = 18;
      ctx.fillStyle = COLORS.player;
      drawPlayer(ctx, p.x, p.y);
      ctx.shadowBlur = 0;

      // Engine glow
      const flicker = Math.sin(frameCountRef.current * 0.5) * 3;
      ctx.globalAlpha = 0.4 + Math.random() * 0.2;
      ctx.fillStyle = "#00ccff";
      ctx.beginPath();
      ctx.moveTo(p.x + 10, p.y + PLAYER_H);
      ctx.lineTo(p.x + PLAYER_W / 2, p.y + PLAYER_H + 12 + flicker);
      ctx.lineTo(p.x + PLAYER_W - 10, p.y + PLAYER_H);
      ctx.fill();
      ctx.globalAlpha = 1;
    }

    // Bullets
    for (const b of bulletsRef.current) {
      if (b.isPlayer) {
        ctx.shadowColor = COLORS.bulletPlayer;
        ctx.shadowBlur = 8;
        ctx.fillStyle = COLORS.bulletPlayer;
      } else {
        ctx.shadowColor = COLORS.bulletEnemy;
        ctx.shadowBlur = 8;
        ctx.fillStyle = COLORS.bulletEnemy;
      }
      ctx.fillRect(b.x, b.y, 4, 10);
      ctx.shadowBlur = 0;
    }

    // Particles
    for (const part of particlesRef.current) {
      const alpha = part.life / part.maxLife;
      ctx.globalAlpha = alpha;
      ctx.fillStyle = part.color;
      ctx.shadowColor = part.color;
      ctx.shadowBlur = 6;
      ctx.beginPath();
      ctx.arc(part.x, part.y, part.size * alpha, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    ctx.shadowBlur = 0;

    // Combo display
    if (gameData.combo > 1 && gameData.state === "playing") {
      ctx.save();
      ctx.font = "bold 22px monospace";
      ctx.textAlign = "center";
      const comboAlpha = Math.min(1, gameData.combo * 0.2);
      ctx.fillStyle = `rgba(255,170,0,${comboAlpha})`;
      ctx.shadowColor = "#ffaa00";
      ctx.shadowBlur = 12;
      ctx.fillText(`${gameData.combo}x COMBO!`, CANVAS_W / 2, 30);
      ctx.restore();
    }

    // Menu overlay
    if (gameData.state === "menu") {
      drawOverlay(ctx, "NEON INVADERS", [
        "Press ENTER or SPACE to start",
        "",
        "← → or A D : Move",
        "SPACE or ↑ : Shoot",
        "",
        `High Score: ${gameData.highScore}`,
      ]);
    }

    // Game over overlay
    if (gameData.state === "gameover") {
      drawOverlay(ctx, "GAME OVER", [
        `Final Score: ${gameData.score}`,
        `Level Reached: ${gameData.level}`,
        `Max Combo: ${gameData.maxCombo}x`,
        "",
        gameData.score >= gameData.highScore && gameData.score > 0 ? "★ NEW HIGH SCORE ★" : `High Score: ${gameData.highScore}`,
        "",
        "Press ENTER or SPACE to restart",
      ]);
    }

    // Victory overlay (level 10)
    if (gameData.state === "victory") {
      drawOverlay(ctx, "★ VICTORY ★", [
        `Final Score: ${gameData.score}`,
        "",
        "You saved the galaxy!",
        "",
        "Press ENTER to play again",
      ]);
    }

    ctx.restore();
  }, [gameData.state, gameData.combo, gameData.score, gameData.highScore, gameData.level, gameData.maxCombo]);

  // ─── Loop driver ──────────────────────────────────────────────────────

  useEffect(() => {
    const loop = () => {
      update();
      render();
      animFrameRef.current = requestAnimationFrame(loop);
    };
    animFrameRef.current = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(animFrameRef.current);
  }, [update, render]);

  // ─── Data-anvil state JSON ────────────────────────────────────────────

  const anvilState = {
    state: gameData.state,
    score: gameData.score,
    highScore: gameData.highScore,
    lives: gameData.lives,
    level: gameData.level,
    combo: gameData.combo,
    maxCombo: gameData.maxCombo,
  };

  return { canvasRef, gameData, anvilState, startGame };
}

// ─── Drawing helpers ──────────────────────────────────────────────────────────

function drawPlayer(ctx: CanvasRenderingContext2D, x: number, y: number) {
  ctx.beginPath();
  ctx.moveTo(x + PLAYER_W / 2, y);           // nose
  ctx.lineTo(x + PLAYER_W - 2, y + PLAYER_H); // right wing tip
  ctx.lineTo(x + PLAYER_W - 8, y + PLAYER_H - 6);
  ctx.lineTo(x + PLAYER_W / 2 + 4, y + PLAYER_H - 10);
  ctx.lineTo(x + PLAYER_W / 2 - 4, y + PLAYER_H - 10);
  ctx.lineTo(x + 8, y + PLAYER_H - 6);
  ctx.lineTo(x + 2, y + PLAYER_H);             // left wing tip
  ctx.closePath();
  ctx.fill();
}

function drawSquid(ctx: CanvasRenderingContext2D, x: number, y: number) {
  const w = INVADER_W, h = INVADER_H;
  ctx.fillRect(x + 4, y, w - 8, 4);          // top
  ctx.fillRect(x + 2, y + 4, w - 4, 4);       // upper body
  ctx.fillRect(x, y + 8, w, 4);               // middle
  ctx.fillRect(x + 2, y + 12, w - 4, 4);      // lower
  ctx.fillRect(x + 6, y + 16, 4, 4);          // legs
  ctx.fillRect(x + w - 10, y + 16, 4, 4);
}

function drawCrab(ctx: CanvasRenderingContext2D, x: number, y: number, frame: number) {
  const w = INVADER_W, h = INVADER_H;
  ctx.fillRect(x + 6, y, w - 12, 4);         // top
  ctx.fillRect(x + 4, y + 4, w - 8, 8);       // body
  if (frame === 0) {
    ctx.fillRect(x, y + 4, 4, 4);             // claws up
    ctx.fillRect(x + w - 4, y + 4, 4, 4);
    ctx.fillRect(x + 6, y + 12, 4, 4);
    ctx.fillRect(x + w - 10, y + 12, 4, 4);
  } else {
    ctx.fillRect(x, y + 8, 4, 4);             // claws down
    ctx.fillRect(x + w - 4, y + 8, 4, 4);
    ctx.fillRect(x + 4, y + 12, 4, 4);
    ctx.fillRect(x + w - 8, y + 12, 4, 4);
  }
}

function drawOctopus(ctx: CanvasRenderingContext2D, x: number, y: number, frame: number) {
  const w = INVADER_W, h = INVADER_H;
  ctx.fillRect(x + 8, y, w - 16, 4);         // top
  ctx.fillRect(x + 4, y + 4, w - 8, 8);       // body
  ctx.fillRect(x + 2, y + 8, w - 4, 4);
  if (frame === 0) {
    ctx.fillRect(x + 6, y + 12, 4, 6);
    ctx.fillRect(x + w - 10, y + 12, 4, 6);
    ctx.fillRect(x + 2, y + 12, 4, 4);
    ctx.fillRect(x + w - 6, y + 12, 4, 4);
  } else {
    ctx.fillRect(x + 8, y + 12, 4, 6);
    ctx.fillRect(x + w - 12, y + 12, 4, 6);
    ctx.fillRect(x, y + 10, 4, 4);
    ctx.fillRect(x + w - 4, y + 10, 4, 4);
  }
}

function drawOverlay(ctx: CanvasRenderingContext2D, title: string, lines: string[]) {
  // Dim overlay
  ctx.fillStyle = "rgba(10,14,39,0.85)";
  ctx.fillRect(0, 0, CANVAS_W, CANVAS_H);

  // Title
  ctx.font = "bold 48px monospace";
  ctx.textAlign = "center";
  ctx.fillStyle = "#00f5ff";
  ctx.shadowColor = "#00f5ff";
  ctx.shadowBlur = 24;
  ctx.fillText(title, CANVAS_W / 2, CANVAS_H / 2 - 80);
  ctx.shadowBlur = 0;

  // Lines
  ctx.font = "18px monospace";
  ctx.fillStyle = "#ffffff";
  lines.forEach((line, i) => {
    if (line.startsWith("★")) {
      ctx.fillStyle = "#ffaa00";
      ctx.shadowColor = "#ffaa00";
      ctx.shadowBlur = 12;
    } else {
      ctx.fillStyle = "#ffffff";
      ctx.shadowBlur = 0;
    }
    ctx.fillText(line, CANVAS_W / 2, CANVAS_H / 2 - 20 + i * 30);
  });
  ctx.shadowBlur = 0;
}

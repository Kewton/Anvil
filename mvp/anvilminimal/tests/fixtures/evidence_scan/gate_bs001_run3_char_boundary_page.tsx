"use client";
import { useEffect, useRef, useState, useCallback } from "react";

// ─── Constants ───────────────────────────────────────────────────────
const W = 800;
const H = 600;
const PLAYER_W = 40;
const PLAYER_H = 20;
const PLAYER_SPEED = 5;
const BULLET_SPEED = 8;
const ENEMY_BULLET_SPEED = 3;
const ENEMY_COLS = 10;
const ENEMY_ROWS = 5;
const ENEMY_W = 30;
const ENEMY_H = 20;
const ENEMY_GAP = 10;
const ENEMY_START_X = 60;
const ENEMY_START_Y = 60;
const ENEMY_STEP = 30;
const PLAYER_Y = H - 60;
const BULLET_MAX = 2;
const POWERUP_CHANCE = 0.002;

type GameState = "start" | "playing" | "game-over" | "victory";

interface Enemy {
  x: number;
  y: number;
  type: number;
  alive: boolean;
  hp: number;
}

interface Bullet {
  x: number;
  y: number;
  speed: number;
  fromPlayer: boolean;
  color: string;
  power: number;
}

interface PowerUp {
  x: number;
  y: number;
  type: "shield" | "rapid" | "triple";
  active: boolean;
  timer: number;
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
  size: number;
  brightness: number;
}

interface GameStateMachine {
  status: GameState;
  playerX: number;
  score: number;
  highScore: number;
  lives: number;
  level: number;
  enemies: Enemy[];
  bullets: Bullet[];
  enemyBullets: Bullet[];
  particles: Particle[];
  powerUps: PowerUp[];
  stars: Star[];
  enemyDir: number;
  enemySpeed: number;
  enemyMoveTimer: number;
  enemyShootTimer: number;
  playerShootCooldown: number;
  rapidFireTimer: number;
  shieldTimer: number;
  tripleShot: boolean;
  flashTimer: number;
  comboCount: number;
  comboTimer: number;
  screenShake: number;
  bossActive: boolean;
  boss: Enemy | null;
  bossDir: number;
  bossShootTimer: number;
  levelTransitionTimer: number;
  invincibleTimer: number;
}

function createStars(): Star[] {
  const stars: Star[] = [];
  for (let i = 0; i < 120; i++) {
    stars.push({
      x: Math.random() * W,
      y: Math.random() * H,
      speed: 0.2 + Math.random() * 1.5,
      size: Math.random() * 2 + 0.5,
      brightness: Math.random(),
    });
  }
  return stars;
}

function createEnemies(level: number): Enemy[] {
  const enemies: Enemy[] = [];
  const types = [0, 1, 2, 3, 4];
  for (let row = 0; row < ENEMY_ROWS; row++) {
    for (let col = 0; col < ENEMY_COLS; col++) {
      enemies.push({
        x: ENEMY_START_X + col * (ENEMY_W + ENEMY_GAP),
        y: ENEMY_START_Y + row * (ENEMY_H + ENEMY_GAP),
        type: types[row % types.length],
        alive: true,
        hp: row < 2 ? 2 : 1,
      });
    }
  }
  return enemies;
}

function createBoss(level: number): Enemy {
  return {
    x: W / 2 - 40,
    y: 40,
    type: 5,
    alive: true,
    hp: 10 + level * 5,
  };
}

function spawnParticles(
  particles: Particle[],
  x: number,
  y: number,
  color: string,
  count: number
) {
  for (let i = 0; i < count; i++) {
    const angle = Math.random() * Math.PI * 2;
    const speed = 1 + Math.random() * 4;
    particles.push({
      x,
      y,
      vx: Math.cos(angle) * speed,
      vy: Math.sin(angle) * speed,
      life: 30 + Math.random() * 30,
      maxLife: 60,
      color,
      size: 1 + Math.random() * 3,
    });
  }
}

function initialGameState(): GameStateMachine {
  return {
    status: "start",
    playerX: W / 2 - PLAYER_W / 2,
    score: 0,
    highScore: 0,
    lives: 3,
    level: 1,
    enemies: createEnemies(1),
    bullets: [],
    enemyBullets: [],
    particles: [],
    powerUps: [],
    stars: createStars(),
    enemyDir: 1,
    enemySpeed: 0.5,
    enemyMoveTimer: 0,
    enemyShootTimer: 0,
    playerShootCooldown: 0,
    rapidFireTimer: 0,
    shieldTimer: 0,
    tripleShot: false,
    flashTimer: 0,
    comboCount: 0,
    comboTimer: 0,
    screenShake: 0,
    bossActive: false,
    boss: null,
    bossDir: 1,
    bossShootTimer: 0,
    levelTransitionTimer: 0,
    invincibleTimer: 0,
  };
}

export default function SpaceInvaders() {
  const [gameState, setGameState] = useState<GameStateMachine>(initialGameState);
  const gameRef = useRef<GameStateMachine>(gameState);
  const keysRef = useRef<Set<string>>(new Set());
  const animFrameRef = useRef<number>(0);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [renderTick, setRenderTick] = useState(0);

  // Sync ref with state
  useEffect(() => {
    gameRef.current = gameState;
  }, [gameState]);

  const updateGameState = useCallback((updater: (gs: GameStateMachine) => void)) => {
    setGameState((prev) => {
      const next = { ...prev };
      updater(next);
      return next;
    });
  }, []);

  // Keyboard handlers
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      keysRef.current.add(e.key);
      if (e.key === " " || e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      keysRef.current.delete(e.key);
    };
    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, []);

  // Game loop
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let lastTime = performance.now();

    const gameLoop = (now: number) => {
      const dt = Math.min((now - lastTime) / 16.67, 3);
      lastTime = now;

      const gs = { ...gameRef.current };
      const keys = keysRef.current;

      // ── Update stars ──
      gs.stars.forEach((star) => {
        star.y += star.speed * dt;
        if (star.y > H) {
          star.y = 0;
          star.x = Math.random() * W;
        }
      });

      if (gs.status === "playing") {
        // ── Player movement ──
        if (keys.has("ArrowLeft") || keys.has("a")) {
          gs.playerX = Math.max(0, gs.playerX - PLAYER_SPEED * dt);
        }
        if (keys.has("ArrowRight") || keys.has("d")) {
          gs.playerX = Math.min(W - PLAYER_W, gs.playerX + PLAYER_SPEED * dt);
        }

        // ── Player shooting ──
        if (gs.playerShootCooldown > 0) gs.playerShootCooldown -= dt;
        if (keys.has(" ") && gs.bullets.length < BULLET_MAX && gs.playerShootCooldown <= 0) {
          const cooldown = gs.rapidFireTimer > 0 ? 5 : 15;
          gs.playerShootCooldown = cooldown;
          gs.bullets.push({
            x: gs.playerX + PLAYER_W / 2 - 2,
            y: PLAYER_Y - 10,
            speed: -BULLET_SPEED,
            fromPlayer: true,
            color: "#00ffff",
            power: gs.rapidFireTimer > 0 ? 2 : 1,
          });
          if (gs.tripleShot) {
            gs.bullets.push({
              x: gs.playerX + PLAYER_W / 2 - 2,
              y: PLAYER_Y - 5,
              speed: -BULLET_SPEED,
              fromPlayer: true,
              color: "#ff00ff",
              power: 1,
            });
          }
        }

        // ── Update player bullets ──
        gs.bullets = gs.bullets.filter((b) => {
          b.y += b.speed * dt;
          return b.y > -10;
        });

        // ── Enemy movement ──
        gs.enemyMoveTimer += dt;
        const aliveEnemies = gs.enemies.filter((e) => e.alive);
        if (aliveEnemies.length > 0) {
          const moveInterval = Math.max(5, 20 - gs.level * 2);
          if (gs.enemyMoveTimer >= moveInterval) {
            gs.enemyMoveTimer = 0;
            let edgeReached = false;
            aliveEnemies.forEach((e) => {
              e.x += gs.enemyDir * (ENEMY_STEP + gs.level * 2);
              if (e.x <= 5 || e.x + ENEMY_W >= W - 5) edgeReached = true;
            });
            if (edgeReached) {
              gs.enemyDir *= -1;
              aliveEnemies.forEach((e) => (e.y += 15));
            }
          }
        }

        // ── Enemy shooting ──
        gs.enemyShootTimer += dt;
        if (gs.enemyShootTimer >= 30 - gs.level * 2 && aliveEnemies.length > 0) {
          gs.enemyShootTimer = 0;
          const shooter = aliveEnemies[Math.floor(Math.random() * aliveEnemies.length)];
          if (shooter) {
            gs.enemyBullets.push({
              x: shooter.x + ENEMY_W / 2,
              y: shooter.y + ENEMY_H,
              speed: ENEMY_BULLET_SPEED + gs.level * 0.3,
              fromPlayer: false,
              color: "#ff4444",
              power: 1,
            });
          }
        }

        // ── Update enemy bullets ──
        gs.enemyBullets = gs.enemyBullets.filter((b) => {
          b.y += b.speed * dt;
          return b.y < H + 10;
        });

        // ── Collision: player bullets vs enemies ──
        gs.bullets.forEach((b) => {
          if (!b.fromPlayer) return;
          gs.enemies.forEach((e) => {
            if (!e.alive) return;
            if (
              b.x > e.x &&
              b.x < e.x + ENEMY_W &&
              b.y > e.y &&
              b.y < e.y + ENEMY_H
            ) {
              e.hp -= b.power;
              if (e.hp <= 0) {
                e.alive = false;
                const points = (e.type + 1) * 10 * (1 + gs.comboCount * 0.1);
                gs.score += Math.floor(points);
                gs.comboCount = Math.min(gs.comboCount + 1, 10);
                gs.comboTimer = 120;
                gs.screenShake = 5;
                spawnParticles(gs.particles, e.x + ENEMY_W / 2, e.y + ENEMY_H / 2, "#ffaa00", 12);
                if (Math.random() < POWERUP_CHANCE) {
                  const types: PowerUp["type"][] = ["shield", "rapid", "triple"];
                  gs.powerUps.push({
                    x: e.x + ENEMY_W / 2,
                    y: e.y,
                    type: types[Math.floor(Math.random() * 3)],
                    active: true,
                    timer: 600,
                  });
                }
              }
              b.y = -100;
            }
          });
        });

        // ── Collision: enemy bullets vs player ──
        if (gs.invincibleTimer > 0) gs.invincibleTimer -= dt;
        gs.enemyBullets = gs.enemyBullets.filter((b) => {
          if (
            b.y > PLAYER_Y &&
            b.y < PLAYER_Y + PLAYER_H &&
            b.x > gs.playerX &&
            b.x < gs.playerX + PLAYER_W
          ) {
            if (gs.shieldTimer <= 0 && gs.invincibleTimer <= 0) {
              gs.lives--;
              gs.screenShake = 10;
              spawnParticles(gs.particles, gs.playerX + PLAYER_W / 2, PLAYER_Y, "#ff0000", 20);
              gs.invincibleTimer = 90;
              if (gs.lives <= 0) {
                gs.status = "game-over";
                if (gs.score > gs.highScore) gs.highScore = gs.score;
              }
            }
            return false;
          }
          return true;
        });

        // ── Collision: enemies reaching player ──
        aliveEnemies.forEach((e) => {
          if (e.y + ENEMY_H >= PLAYER_Y) {
            gs.status = "game-over";
            if (gs.score > gs.highScore) gs.highScore = gs.score;
          }
        });

        // ── Level complete ──
        if (aliveEnemies.length === 0) {
          gs.level++;
          gs.enemies = createEnemies(gs.level);
          gs.enemyDir = 1;
          gs.bullets = [];
          gs.enemyBullets = [];
          gs.levelTransitionTimer = 180;
          if (gs.level > 10) {
            gs.bossActive = true;
            gs.boss = createBoss(gs.level);
          }
        }

        // ── Boss logic ──
        if (gs.bossActive && gs.boss) {
          gs.boss.x += gs.bossDir * (2 + gs.level * 0.5) * dt;
          if (gs.boss.x <= 5 || gs.boss.x + ENEMY_W >= W - 5) {
            gs.bossDir *= -1;
          }
          gs.bossShootTimer += dt;
          if (gs.bossShootTimer >= 20) {
            gs.bossShootTimer = 0;
            gs.enemyBullets.push({
              x: gs.boss.x + ENEMY_W / 2,
              y: gs.boss.y + ENEMY_H,
              speed: ENEMY_BULLET_SPEED * 1.5,
              fromPlayer: false,
              color: "#ff00ff",
              power: 2,
            });
          }
        }

        // ── Update power-ups ──
        gs.powerUps = gs.powerUps.filter((p) => {
          p.y += 1.5 * dt;
          p.timer -= dt;
          if (p.y > H || p.timer <= 0) return false;
          if (
            p.x + 10 > gs.playerX &&
            p.x - 10 < gs.playerX + PLAYER_W &&
            p.y > PLAYER_Y &&
            p.y < PLAYER_Y + PLAYER_H
          ) {
            if (p.type === "shield") gs.shieldTimer = 600;
            if (p.type === "rapid") gs.rapidFireTimer = 400;
            if (p.type === "triple") gs.tripleShot = true;
            spawnParticles(gs.particles, p.x, p.y, "#00ff00", 8);
            return false;
          }
          return true;
        });

        // ── Timers ──
        if (gs.shieldTimer > 0) gs.shieldTimer -= dt;
        if (gs.rapidFireTimer > 0) gs.rapidFireTimer -= dt;
        if (gs.comboTimer > 0) {
          gs.comboTimer -= dt;
          if (gs.comboTimer <= 0) gs.comboCount = 0;
        }
        if (gs.screenShake > 0) gs.screenShake -= dt;
        if (gs.flashTimer > 0) gs.flashTimer -= dt;
        if (gs.levelTransitionTimer > 0) gs.levelTransitionTimer -= dt;
      }

      // ── Update particles ──
      gs.particles = gs.particles.filter((p) => {
        p.x += p.vx * dt;
        p.y += p.vy * dt;
        p.life -= dt;
        return p.life > 0;
      });

      // ── Render ──
      ctx.save();
      if (gs.screenShake > 0) {
        ctx.translate(
          (Math.random() - 0.5) * gs.screenShake * 2,
          (Math.random() - 0.5) * gs.screenShake * 2
        );
      }

      // Background
      const grad = ctx.createLinearGradient(0, 0, 0, H);
      grad.addColorStop(0, "#0a0a2e");
      grad.addColorStop(1, "#1a0a3e");
      ctx.fillStyle = grad;
      ctx.fillRect(0, 0, W, H);

      // Stars
      gs.stars.forEach((star) => {
        const alpha = 0.3 + star.brightness * 0.7;
        ctx.fillStyle = `rgba(255,255,255,${alpha})`;
        ctx.beginPath();
        ctx.arc(star.x, star.y, star.size, 0, Math.PI * 2);
        ctx.fill();
      });

      // Grid lines
      ctx.strokeStyle = "rgba(0,255,255,0.05)";
      ctx.lineWidth = 1;
      for (let i = 0; i < W; i += 40) {
        ctx.beginPath();
        ctx.moveTo(i, 0);
        ctx.lineTo(i, H);
        ctx.stroke();
      }
      for (let i = 0; i < H; i += 40) {
        ctx.beginPath();
        ctx.moveTo(0, i);
        ctx.lineTo(W, i);
        ctx.stroke();
      }

      if (gs.status === "start") {
        // Title screen
        ctx.textAlign = "center";
        ctx.fillStyle = "#00ffff";
        ctx.font = "bold 48px monospace";
        ctx.shadowColor = "#00ffff";
        ctx.shadowBlur = 20;
        ctx.fillText("SPACE INVADERS", W / 2, H / 2 - 80);
        ctx.shadowBlur = 0;
        ctx.font = "18px monospace";
        ctx.fillStyle = "#88ffff";
        ctx.fillText("← → を移動 | SPACE で弾発射", W / 2, H / 2);
        ctx.font = "14px monospace";
        ctx.fillStyle = "#666688";
        ctx.fillText("10段階のレベルとボス戦付き", W / 2, H / 2 + 40);
        ctx.fillText("パワーアップアイテムも登場", W / 2, H / 2 + 60);
      } else if (gs.status === "game-over") {
        ctx.textAlign = "center";
        ctx.fillStyle = "#ff4444";
        ctx.font = "bold 48px monospace";
        ctx.shadowColor = "#ff0000";
        ctx.shadowBlur = 20;
        ctx.fillText("GAME OVER", W / 2, H / 2 - 40);
        ctx.shadowBlur = 0;
        ctx.font = "24px monospace";
        ctx.fillStyle = "#ffaa00";
        ctx.fillText(`スコア: ${gs.score}`, W / 2, H / 2 + 20);
        if (gs.score >= gs.highScore && gs.score > 0) {
          ctx.fillStyle = "#ffff00";
          ctx.font = "18px monospace";
          ctx.fillText("★ NEW HIGH SCORE ★", W / 2, H / 2 + 50);
        }
      } else if (gs.status === "victory") {
        ctx.textAlign = "center";
        ctx.fillStyle = "#00ff00";
        ctx.font = "bold 48px monospace";
        ctx.shadowColor = "#00ff00";
        ctx.shadowBlur = 20;
        ctx.fillText("VICTORY!", W / 2, H / 2 - 40);
        ctx.shadowBlur = 0;
        ctx.font = "24px monospace";
        ctx.fillStyle = "#ffaa00";
        ctx.fillText(`最終スコア: ${gs.score}`, W / 2, H / 2 + 20);
      } else {
        // Playing state

        // Level transition
        if (gs.levelTransitionTimer > 0) {
          ctx.textAlign = "center";
          ctx.fillStyle = `rgba(0,255,255,${gs.levelTransitionTimer / 180})`;
          ctx.font = "bold 36px monospace";
          ctx.fillText(`LEVEL ${gs.level}`, W / 2, H / 2);
        }

        // Draw enemies
        gs.enemies.forEach((e) => {
          if (!e.alive) return;
          const colors = ["#ff4444", "#ff8844", "#ffcc44", "#44ff44", "#4488ff"];
          ctx.fillStyle = colors[e.type % colors.length];
          ctx.shadowColor = colors[e.type % colors.length];
          ctx.shadowBlur = 8;
          // Body
          ctx.fillRect(e.x + 5, e.y + 5, ENEMY_W - 10, ENEMY_H - 10);
          // Eyes
          ctx.fillStyle = "#000";
          ctx.fillRect(e.x + 8, e.y + 8, 4, 4);
          ctx.fillRect(e.x + ENEMY_W - 12, e.y + 8, 4, 4);
          // Antenna
          ctx.fillRect(e.x + ENEMY_W / 2 - 1, e.y, 2, 5);
          ctx.shadowBlur = 0;
        });

        // Boss
        if (gs.bossActive && gs.boss) {
          const e = gs.boss;
          ctx.fillStyle = "#ff00ff";
          ctx.shadowColor = "#ff00ff";
          ctx.shadowBlur = 15;
          ctx.fillRect(e.x, e.y, ENEMY_W * 1.5, ENEMY_H * 1.5);
          ctx.fillStyle = "#000";
          ctx.fillRect(e.x + 10, e.y + 10, 8, 8);
          ctx.fillRect(e.x + ENEMY_W + 5, e.y + 10, 8, 8);
          ctx.shadowBlur = 0;
          // Boss HP bar
          ctx.fillStyle = "#333";
          ctx.fillRect(e.x, e.y - 10, ENEMY_W * 1.5, 4);
          ctx.fillStyle = "#ff00ff";
          ctx.fillRect(e.x, e.y - 10, (ENEMY_W * 1.5) * (e.hp / (10 + gs.level * 5)), 4);
        }

        // Draw player
        if (gs.invincibleTimer > 0 && Math.floor(gs.invincibleTimer) % 4 < 2) {
          // Blink
        } else {
          ctx.fillStyle = "#00ffff";
          ctx.shadowColor = "#00ffff";
          ctx.shadowBlur = 10;
          // Ship body
          ctx.beginPath();
          ctx.moveTo(gs.playerX + PLAYER_W / 2, PLAYER_Y);
          ctx.lineTo(gs.playerX, PLAYER_Y + PLAYER_H);
          ctx.lineTo(gs.playerX + PLAYER_W, PLAYER_Y + PLAYER_H);
          ctx.closePath();
          ctx.fill();
          // Cockpit
          ctx.fillStyle = "#ffffff";
          ctx.beginPath();
          ctx.arc(gs.playerX + PLAYER_W / 2, PLAYER_Y + 8, 4, 0, Math.PI * 2);
          ctx.fill();
          // Shield visual
          if (gs.shieldTimer > 0) {
            ctx.strokeStyle = `rgba(0,255,100,${0.3 + Math.sin(now * 0.01) * 0.2})`;
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.arc(gs.playerX + PLAYER_W / 2, PLAYER_Y + PLAYER_H / 2, 30, 0, Math.PI * 2);
            ctx.stroke();
          }
          ctx.shadowBlur = 0;
        }

        // Draw player bullets
        gs.bullets.forEach((b) => {
          ctx.fillStyle = b.color;
          ctx.shadowColor = b.color;
          ctx.shadowBlur = 8;
          ctx.fillRect(b.x, b.y, 4, 12);
          ctx.shadowBlur = 0;
        });

        // Draw enemy bullets
        gs.enemyBullets.forEach((b) => {
          ctx.fillStyle = b.color;
          ctx.shadowColor = b.color;
          ctx.shadowBlur = 6;
          ctx.beginPath();
          ctx.arc(b.x, b.y, 4, 0, Math.PI * 2);
          ctx.fill();
          ctx.shadowBlur = 0;
        });

        // Draw power-ups
        gs.powerUps.forEach((p) => {
          const colors = { shield: "#00ff00", rapid: "#ffff00", triple: "#ff00ff" };
          ctx.fillStyle = colors[p.type];
          ctx.shadowColor = colors[p.type];
          ctx.shadowBlur = 10;
          ctx.beginPath();
          ctx.arc(p.x, p.y, 8, 0, Math.PI * 2);
          ctx.fill();
          ctx.fillStyle = "#000";
          ctx.font = "10px monospace";
          ctx.textAlign = "center";
          ctx.fillText(p.type[0].toUpperCase(), p.x, p.y + 4);
          ctx.shadowBlur = 0;
        });

        // Draw particles
        gs.particles.forEach((p) => {
          const alpha = p.life / p.maxLife;
          ctx.fillStyle = p.color.replace(")", `,${alpha})`).replace("rgb", "rgba");
          ctx.beginPath();
          ctx.arc(p.x, p.y, p.size * alpha, 0, Math.PI * 2);
          ctx.fill();
        });

        // HUD
        ctx.fillStyle = "#ffffff";
        ctx.font = "16px monospace";
        ctx.textAlign = "left";
        ctx.fillText(`SCORE: ${gs.score}`, 20, 25);
        ctx.textAlign = "right";
        ctx.fillText(`LEVEL: ${gs.level}`, W - 20, 25);
        ctx.textAlign = "center";
        // Lives
        for (let i = 0; i < gs.lives; i++) {
          ctx.fillStyle = "#00ffff";
          ctx.beginPath();
          ctx.moveTo(W / 2 - 40 + i * 25, 18);
          ctx.lineTo(W / 2 - 45 + i * 25, 28);
          ctx.lineTo(W / 2 - 35 + i * 25, 28);
          ctx.closePath();
          ctx.fill();
        }
        // Combo
        if (gs.comboCount > 1) {
          ctx.fillStyle = `rgba(255,200,0,${gs.comboTimer / 120})`;
          ctx.font = "bold 14px monospace";
          ctx.textAlign = "center";
          ctx.fillText(`COMBO x${gs.comboCount}`, W / 2, 45);
        }
      }

      ctx.restore();
      setRenderTick((t) => t + 1);

      animFrameRef.current = requestAnimationFrame(gameLoop);
    };

    animFrameRef.current = requestAnimationFrame(gameLoop);
    return () => cancelAnimationFrame(animFrameRef.current);
  }, []);

  const startGame = () => {
    updateGameState((gs) => {
      gs.status = "playing";
      gs.playerX = W / 2 - PLAYER_W / 2;
      gs.score = 0;
      gs.lives = 3;
      gs.level = 1;
      gs.enemies = createEnemies(1);
      gs.bullets = [];
      gs.enemyBullets = [];
      gs.powerUps = [];
      gs.enemyDir = 1;
      gs.comboCount = 0;
      gs.bossActive = false;
      gs.boss = null;
      gs.invincibleTimer = 60;
    });
  };

  const restartGame = () => {
    updateGameState((gs) => {
      gs.status = "playing";
      gs.playerX = W / 2 - PLAYER_W / 2;
      gs.score = 0;
      gs.lives = 3;
      gs.level = 1;
      gs.enemies = createEnemies(1);
      gs.bullets = [];
      gs.enemyBullets = [];
      gs.powerUps = [];
      gs.enemyDir = 1;
      gs.comboCount = 0;
      gs.bossActive = false;
      gs.boss = null;
      gs.invincibleTimer = 60;
    });
  };

  const stateJson = JSON.stringify({
    status: gameState.status,
    playerX: Math.round(gameState.playerX),
    score: gameState.score,
    lives: gameState.lives,
    level: gameState.level,
    combo: gameState.comboCount,
  });

  return (
    <div className="flex flex-col items-center justify-center min-h-screen bg-black">
      <div className="relative">
        <canvas
          ref={canvasRef}
          width={W}
          height={H}
          className="border-2 border-cyan-500 rounded-lg shadow-lg shadow-cyan-500/20"
          style={{ imageRendering: "pixelated" }}
        />
        {gameState.status === "start" && (
          <div className="absolute inset-0 flex items-center justify-center">
            <button
              data-anvil-action="primary"
              onClick={startGame}
              className="px-8 py-4 bg-cyan-600 hover:bg-cyan-500 text-white font-bold text-xl rounded-lg shadow-lg shadow-cyan-500/50 transition-all duration-200 hover:scale-105"
            >
              ▶ START GAME
            </button>
          </div>
        )}
        {(gameState.status === "game-over" || gameState.status === "victory") && (
          <div className="absolute inset-0 flex items-center justify-center">
            <button
              data-anvil-action="restart"
              onClick={restartGame}
              className="px-8 py-4 bg-cyan-600 hover:bg-cyan-500 text-white font-bold text-xl rounded-lg shadow-lg shadow-cyan-500/50 transition-all duration-200 hover:scale-105"
            >
              ↻ RESTART
            </button>
          </div>
        )}
      </div>
      <div
        className="mt-4 text-cyan-400 font-mono text-sm"
        data-anvil-state={stateJson}
      />
      <div className="mt-2 text-gray-500 text-xs font-mono">
        ← → で移動 | SPACE で弾発射
      </div>
    </div>
  );
}

interface Vector2D {
  x: number;
  y: number;
}

interface Player {
  position: Vector2D;
  width: number;
  height: number;
  speed: number;
  lives: number;
  invincible: boolean;
  invincibleTimer: number;
}

interface Invader {
  position: Vector2D;
  type: number;
  alive: boolean;
  width: number;
  height: number;
  points: number;
}

interface Bullet {
  position: Vector2D;
  velocity: Vector2D;
  isPlayerBullet: boolean;
  width: number;
  height: number;
}

interface Particle {
  position: Vector2D;
  velocity: Vector2D;
  life: number;
  maxLife: number;
  color: string;
  size: number;
}

export type GameState = "idle" | "playing" | "gameover";

export interface GameStatus {
  state: GameState;
  score: number;
  lives: number;
  wave: number;
  highScore: number;
  playerPosition: Vector2D;
  invaderCount: number;
}

const CANVAS_WIDTH = 800;
const CANVAS_HEIGHT = 600;
const PLAYER_SPEED = 5;
const BULLET_SPEED = 7;
const INVADER_BULLET_SPEED = 3;
const INVADERS_PER_ROW = 10;
const INVADER_ROWS = 4;
const INVADER_WIDTH = 32;
const INVADER_HEIGHT = 24;
const INVADER_PADDING = 8;
const PLAYER_WIDTH = 40;
const PLAYER_HEIGHT = 20;

export class GameEngine {
  private canvas: HTMLCanvasElement | null = null;
  private ctx: CanvasRenderingContext2D | null = null;

  private player: Player;
  private invaders: Invader[];
  private bullets: Bullet[];
  private particles: Particle[];
  private keys: Set<string>;

  private gameState: GameState;
  private score: number;
  private lives: number;
  private wave: number;
  private highScore: number;

  private invaderDirection: number;
  private invaderSpeed: number;
  private lastInvaderMove: number;
  private invaderMoveInterval: number;

  private lastBulletTime: number;
  private bulletCooldown: number;

  private animationFrameId: number | null = null;
  private lastUpdateTime: number;
  private fixedStep: number = 1000 / 60; // 60 FPS fixed step

  constructor() {
    this.player = {
      position: { x: CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2, y: CANVAS_HEIGHT - 50 },
      width: PLAYER_WIDTH,
      height: PLAYER_HEIGHT,
      speed: PLAYER_SPEED,
      lives: 3,
      invincible: false,
      invincibleTimer: 0
    };

    this.invaders = [];
    this.bullets = [];
    this.particles = [];
    this.keys = new Set();

    this.gameState = "idle";
    this.score = 0;
    this.lives = 3;
    this.wave = 1;
    this.highScore = 0;

    this.invaderDirection = 1;
    this.invaderSpeed = 0.5;
    this.lastInvaderMove = 0;
    this.invaderMoveInterval = 800; // ms between invader moves

    this.lastBulletTime = 0;
    this.bulletCooldown = 250; // ms between player bullets

    this.reset();
  }

  reset(): void {
    this.player.position.x = CANVAS_WIDTH / 2 - PLAYER_WIDTH / 2;
    this.player.lives = 3;
    this.player.invincible = false;

    this.invaders = [];
    for (let row = 0; row < INVADER_ROWS; row++) {
      for (let col = 0; col < INVADERS_PER_ROW; col++) {
        const x = 50 + col * (INVADER_WIDTH + INVADER_PADDING);
        const y = 80 + row * (INVADER_HEIGHT + INVADER_PADDING);
        this.invaders.push({
          position: { x, y },
          type: row % 3,
          alive: true,
          width: INVADER_WIDTH,
          height: INVADER_HEIGHT,
          points: (3 - row) * 10
        });
      }
    }

    this.bullets = [];
    this.particles = [];

    this.gameState = "idle";
    this.score = 0;
    this.wave = 1;

    this.invaderDirection = 1;
    this.invaderSpeed = 0.5 + (this.wave - 1) * 0.2;
    this.lastInvaderMove = 0;
    this.invaderMoveInterval = Math.max(200, 800 - (this.wave - 1) * 100);

    this.lastBulletTime = 0;
  }

  start(): void {
    if (this.gameState === "idle") {
      this.gameState = "playing";
    }
  }

  update(deltaTime: number): void {
    if (this.gameState !== "playing") return;

    this.updatePlayer();
    this.updateBullets();
    this.updateInvaders(deltaTime);
    this.updateParticles();
    this.checkCollisions();
    this.checkWaveComplete();
  }

  private updatePlayer(): void {
    if (this.keys.has("ArrowLeft") || this.keys.has("a")) {
      this.player.position.x = Math.max(0, this.player.position.x - this.player.speed);
    }
    if (this.keys.has("ArrowRight") || this.keys.has("d")) {
      this.player.position.x = Math.min(CANVAS_WIDTH - this.player.width, this.player.position.x + this.player.speed);
    }

    if (this.player.invincible) {
      this.player.invincibleTimer -= 1000 / 60; // assume ~60fps
      if (this.player.invincibleTimer <= 0) {
        this.player.invincible = false;
      }
    }

    const now = Date.now();
    if ((this.keys.has(" ") || this.keys.has("Space")) && now - this.lastBulletTime > this.bulletCooldown) {
      this.bullets.push({
        position: {
          x: this.player.position.x + this.player.width / 2 - 2,
          y: this.player.position.y
        },
        velocity: { x: 0, y: -BULLET_SPEED },
        isPlayerBullet: true,
        width: 4,
        height: 12
      });
      this.lastBulletTime = now;
    }
  }

  private updateBullets(): void {
    for (let i = this.bullets.length - 1; i >= 0; i--) {
      const bullet = this.bullets[i];
      bullet.position.x += bullet.velocity.x;
      bullet.position.y += bullet.velocity.y;

      if (bullet.position.y < 0 || bullet.position.y > CANVAS_HEIGHT) {
        this.bullets.splice(i, 1);
      }
    }

    // Invader shooting
    if (Math.random() < 0.02 && this.invaders.some(inv => inv.alive)) {
      const aliveInvaders = this.invaders.filter(inv => inv.alive);
      if (aliveInvaders.length > 0) {
        const shooter = aliveInvaders[Math.floor(Math.random() * aliveInvaders.length)];
        this.bullets.push({
          position: {
            x: shooter.position.x + shooter.width / 2 - 2,
            y: shooter.position.y + shooter.height
          },
          velocity: { x: 0, y: INVADER_BULLET_SPEED },
          isPlayerBullet: false,
          width: 4,
          height: 12
        });
      }
    }
  }

  private updateInvaders(deltaTime: number): void {
    const now = Date.now();
    if (now - this.lastInvaderMove > this.invaderMoveInterval) {
      let shouldDropDown = false;

      for (const invader of this.invaders) {
        if (!invader.alive) continue;

        const newX = invader.position.x + this.invaderSpeed * this.invaderDirection;
        if (newX < 20 || newX + invader.width > CANVAS_WIDTH - 20) {
          shouldDropDown = true;
          break;
        }
      }

      if (shouldDropDown) {
        this.invaderDirection *= -1;
        for (const invader of this.invaders) {
          if (invader.alive) {
            invader.position.y += 20;
          }
        }
      } else {
        for (const invader of this.invaders) {
          if (invader.alive) {
            invader.position.x += this.invaderSpeed * this.invaderDirection;
          }
        }
      }

      this.lastInvaderMove = now;
    }
  }

  private updateParticles(): void {
    for (let i = this.particles.length - 1; i >= 0; i--) {
      const particle = this.particles[i];
      particle.position.x += particle.velocity.x;
      particle.position.y += particle.velocity.y;
      particle.life -= 16; // assume ~60fps

      if (particle.life <= 0) {
        this.particles.splice(i, 1);
      }
    }
  }

  private checkCollisions(): void {
    for (let i = this.bullets.length - 1; i >= 0; i--) {
      const bullet = this.bullets[i];

      if (bullet.isPlayerBullet) {
        // Check invader collisions
        for (const invader of this.invaders) {
          if (!invader.alive) continue;

          if (this.rectIntersect(
            bullet.position.x, bullet.position.y, bullet.width, bullet.height,
            invader.position.x, invader.position.y, invader.width, invader.height
          )) {
            invader.alive = false;
            this.bullets.splice(i, 1);
            this.score += invader.points;

            // Spawn particles
            for (let j = 0; j < 8; j++) {
              const angle = Math.random() * Math.PI * 2;
              const speed = 1 + Math.random() * 3;
              this.particles.push({
                position: { x: invader.position.x + invader.width / 2, y: invader.position.y + invader.height / 2 },
                velocity: { x: Math.cos(angle) * speed, y: Math.sin(angle) * speed },
                life: 500,
                maxLife: 500,
                color: ["#ef4444", "#f59e0b", "#a855f7"][invader.type % 3],
                size: 2 + Math.random() * 3
              });
            }

            if (this.score > this.highScore) {
              this.highScore = this.score;
            }
            break;
          }
        }
      } else {
        // Check player collisions
        if (!this.player.invincible && this.rectIntersect(
          bullet.position.x, bullet.position.y, bullet.width, bullet.height,
          this.player.position.x, this.player.position.y, this.player.width, this.player.height
        )) {
          this.bullets.splice(i, 1);
          this.lives--;

          if (this.lives <= 0) {
            this.gameState = "gameover";
            if (this.score > this.highScore) {
              this.highScore = this.score;
            }
          } else {
            this.player.invincible = true;
            this.player.invincibleTimer = 2000; // 2 seconds invincibility
          }
        }
      }
    }

    // Check if invaders reached player level
    for (const invader of this.invaders) {
      if (invader.alive && invader.position.y + invader.height >= this.player.position.y) {
        this.gameState = "gameover";
        break;
      }
    }
  }

  private checkWaveComplete(): void {
    const aliveCount = this.invaders.filter(inv => inv.alive).length;
    if (aliveCount === 0) {
      this.wave++;
      this.reset();
      // Keep score and highScore, increase difficulty
      this.invaderSpeed += 0.3;
      this.invaderMoveInterval = Math.max(200, this.invaderMoveInterval - 150);
    }
  }

  private rectIntersect(x1: number, y1: number, w1: number, h1: number,
                        x2: number, y2: number, w2: number, h2: number): boolean {
    return !(x2 > x1 + w1 ||
             x2 + w2 < x1 ||
             y2 > y1 + h1 ||
             y2 + h2 < y1);
  }

  getState(): GameStatus {
    const aliveCount = this.invaders.filter(inv => inv.alive).length;
    return {
      state: this.gameState,
      score: this.score,
      lives: this.lives,
      wave: this.wave,
      highScore: this.highScore,
      playerPosition: { ...this.player.position },
      invaderCount: aliveCount
    };
  }

  setKey(key: string, pressed: boolean): void {
    if (pressed) {
      this.keys.add(key);
    } else {
      this.keys.delete(key);
    }

    if (key === "Enter" && pressed) {
      if (this.gameState === "idle") {
        this.start();
      } else if (this.gameState === "gameover") {
        this.reset();
        this.start();
      }
    }
  }

  draw(ctx: CanvasRenderingContext2D): void {
    ctx.clearRect(0, 0, CANVAS_WIDTH, CANVAS_HEIGHT);

    // Draw player
    if (!this.player.invincible || Math.floor(Date.now() / 100) % 2 === 0) {
      ctx.fillStyle = "#22d3ee";
      ctx.shadowColor = "#22d3ee";
      ctx.shadowBlur = 15;
      ctx.fillRect(
        this.player.position.x,
        this.player.position.y,
        this.player.width,
        this.player.height
      );
      // Ship detail
      ctx.fillStyle = "#a5f3fc";
      ctx.fillRect(
        this.player.position.x + 16,
        this.player.position.y - 4,
        8,
        8
      );
    }

    // Draw invaders
    for (const invader of this.invaders) {
      if (!invader.alive) continue;

      const colors = ["#ef4444", "#f59e0b", "#a855f7"];
      ctx.fillStyle = colors[invader.type % 3];
      ctx.shadowColor = colors[invader.type % 3];
      ctx.shadowBlur = 10;

      // Simple invader shape
      const cx = invader.position.x + invader.width / 2;
      const cy = invader.position.y + invader.height / 2;
      ctx.fillRect(invader.position.x, invader.position.y, invader.width, invader.height);
      ctx.fillRect(invader.position.x + 4, invader.position.y - 6, invader.width - 8, 6);

      // Eyes
      ctx.fillStyle = "#fff";
      ctx.shadowBlur = 0;
      ctx.fillRect(cx - 6, cy - 2, 3, 3);
      ctx.fillRect(cx + 3, cy - 2, 3, 3);
    }

    // Draw bullets
    for (const bullet of this.bullets) {
      const color = bullet.isPlayerBullet ? "#22d3ee" : "#ef4444";
      ctx.fillStyle = color;
      ctx.shadowColor = color;
      ctx.shadowBlur = 8;
      ctx.fillRect(bullet.position.x, bullet.position.y, bullet.width, bullet.height);
    }

    // Draw particles
    for (const particle of this.particles) {
      const alpha = particle.life / particle.maxLife;
      ctx.fillStyle = particle.color + Math.floor(alpha * 255).toString(16).padStart(2, "0");
      ctx.shadowBlur = 0;
      ctx.fillRect(particle.position.x, particle.position.y, particle.size, particle.size);
    }

    // Draw HUD
    ctx.fillStyle = "#fff";
    ctx.font = "16px monospace";
    ctx.shadowBlur = 0;
    ctx.textAlign = "left";
    ctx.fillText(`SCORE: ${this.score}`, 20, 30);
    ctx.textAlign = "center";
    ctx.fillText(`WAVE: ${this.wave}`, CANVAS_WIDTH / 2, 30);
    ctx.textAlign = "right";
    ctx.fillText(`LIVES: ${this.lives}`, CANVAS_WIDTH - 20, 30);

    // Game state messages
    if (this.gameState === "idle") {
      ctx.fillStyle = "#fff";
      ctx.font = "48px monospace";
      ctx.textAlign = "center";
      ctx.shadowColor = "#22d3ee";
      ctx.shadowBlur = 20;
      ctx.fillText("PRESS ENTER TO START", CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2);
    } else if (this.gameState === "gameover") {
      ctx.fillStyle = "#ef4444";
      ctx.font = "64px monospace";
      ctx.textAlign = "center";
      ctx.shadowColor = "#ef4444";
      ctx.shadowBlur = 30;
      ctx.fillText("GAME OVER", CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 - 30);

      ctx.fillStyle = "#fff";
      ctx.font = "24px monospace";
      ctx.shadowBlur = 0;
      ctx.fillText(`FINAL SCORE: ${this.score}`, CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 20);
      ctx.fillText("PRESS ENTER TO RESTART", CANVAS_WIDTH / 2, CANVAS_HEIGHT / 2 + 60);
    }
  }

  init(canvas: HTMLCanvasElement): void {
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d");

    if (!this.ctx) return;

    const gameLoop = (timestamp: number) => {
      if (!this.lastUpdateTime) {
        this.lastUpdateTime = timestamp;
      }

      const deltaTime = timestamp - this.lastUpdateTime;
      this.lastUpdateTime = timestamp;

      // Fixed step update
      let remaining = deltaTime;
      while (remaining >= this.fixedStep) {
        this.update(this.fixedStep);
        remaining -= this.fixedStep;
      }

      if (this.canvas && this.ctx) {
        this.draw(this.ctx);
      }

      this.animationFrameId = requestAnimationFrame(gameLoop);
    };

    this.animationFrameId = requestAnimationFrame(gameLoop);
  }

  handleKeyDown(e: KeyboardEvent): void {
    this.keys.add(e.key);
    // Start/restart game on Enter when not playing
    if (e.key === "Enter" && this.gameState !== "playing") {
      if (this.gameState === "gameover") {
        this.reset();
      }
      this.startWave();
    }
  }

  handleKeyUp(e: KeyboardEvent): void {
    this.keys.delete(e.key);
  }

  cleanup(): void {
    if (this.animationFrameId !== null) {
      cancelAnimationFrame(this.animationFrameId);
      this.animationFrameId = null;
    }
    this.keys.clear();
  }
}

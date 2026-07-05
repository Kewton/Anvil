"use client";

import React, { useState, useEffect, useRef } from "react";

// Types
type GameState = "start" | "playing" | "paused" | "gameover" | "victory" | "campaign_clear";

interface Projectile {
  x: number;
  y: number;
  width: number;
  height: number;
  dy: number;
  isPlayer: boolean;
  type: "standard" | "triple" | "laser-beam";
}

interface Invader {
  id: number;
  x: number;
  y: number;
  width: number;
  height: number;
  type: number; // 1 = regular, 2 = swift, 3 = heavy, 4 = elite
  points: number;
  hp: number;
  maxHp: number;
  shield: boolean;
}

interface Boss {
  x: number;
  y: number;
  width: number;
  height: number;
  hp: number;
  maxHp: number;
  direction: number;
  laserCooldown: number;
  active: boolean;
}

interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  color: string;
  size: number;
  alpha: number;
  decay: number;
}

interface Star {
  x: number;
  y: number;
  size: number;
  speed: number;
}

interface BunkerBlock {
  x: number;
  y: number;
  width: number;
  height: number;
  hp: number; // Max 4, changes color as it takes damage
}

interface PowerUp {
  x: number;
  y: number;
  width: number;
  height: number;
  type: "triple" | "laser-beam" | "shield" | "freeze" | "bomb";
  dy: number;
}

interface HighScore {
  name: string;
  score: number;
  level: number;
  date: string;
}

// Audio Synthesizer (No external assets required!)
const playSound = (type: "laser" | "explosion" | "powerup" | "hit" | "bossLaser" | "gameover" | "shield") => {
  if (typeof window === "undefined") return;
  try {
    const AudioContext = window.AudioContext || (window as any).webkitAudioContext;
    if (!AudioContext) return;
    const ctx = new AudioContext();
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();

    osc.connect(gain);
    gain.connect(ctx.destination);

    if (type === "laser") {
      osc.type = "sawtooth";
      osc.frequency.setValueAtTime(600, ctx.currentTime);
      osc.frequency.exponentialRampToValueAtTime(100, ctx.currentTime + 0.12);
      gain.gain.setValueAtTime(0.08, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.12);
      osc.start();
      osc.stop(ctx.currentTime + 0.12);
    } else if (type === "explosion") {
      osc.type = "sawtooth";
      osc.frequency.setValueAtTime(120, ctx.currentTime);
      osc.frequency.exponentialRampToValueAtTime(20, ctx.currentTime + 0.35);
      gain.gain.setValueAtTime(0.25, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.35);
      osc.start();
      osc.stop(ctx.currentTime + 0.35);
    } else if (type === "hit") {
      osc.type = "triangle";
      osc.frequency.setValueAtTime(250, ctx.currentTime);
      osc.frequency.exponentialRampToValueAtTime(50, ctx.currentTime + 0.08);
      gain.gain.setValueAtTime(0.12, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.08);
      osc.start();
      osc.stop(ctx.currentTime + 0.08);
    } else if (type === "powerup") {
      osc.type = "sine";
      osc.frequency.setValueAtTime(250, ctx.currentTime);
      osc.frequency.linearRampToValueAtTime(500, ctx.currentTime + 0.1);
      osc.frequency.linearRampToValueAtTime(950, ctx.currentTime + 0.25);
      gain.gain.setValueAtTime(0.12, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.25);
      osc.start();
      osc.stop(ctx.currentTime + 0.25);
    } else if (type === "bossLaser") {
      osc.type = "sawtooth";
      osc.frequency.setValueAtTime(80, ctx.currentTime);
      osc.frequency.linearRampToValueAtTime(220, ctx.currentTime + 0.45);
      gain.gain.setValueAtTime(0.15, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.45);
      osc.start();
      osc.stop(ctx.currentTime + 0.45);
    } else if (type === "gameover") {
      osc.type = "sawtooth";
      osc.frequency.setValueAtTime(220, ctx.currentTime);
      osc.frequency.linearRampToValueAtTime(110, ctx.currentTime + 0.4);
      osc.frequency.linearRampToValueAtTime(40, ctx.currentTime + 0.9);
      gain.gain.setValueAtTime(0.2, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.9);
      osc.start();
      osc.stop(ctx.currentTime + 0.9);
    } else if (type === "shield") {
      osc.type = "sine";
      osc.frequency.setValueAtTime(400, ctx.currentTime);
      osc.frequency.setValueAtTime(600, ctx.currentTime + 0.05);
      osc.frequency.setValueAtTime(800, ctx.currentTime + 0.1);
      gain.gain.setValueAtTime(0.1, ctx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.01, ctx.currentTime + 0.15);
      osc.start();
      osc.stop(ctx.currentTime + 0.15);
    }
  } catch (err) {
    // Audio Context blocked
  }
};

export default function GamePage() {
  // Game state hooks
  const [gameState, setGameState] = useState<GameState>("start");
  const [score, setScore] = useState(0);
  const [level, setLevel] = useState(1);
  const [lives, setLives] = useState(3);
  const [combo, setCombo] = useState(0);
  const [maxCombo, setMaxCombo] = useState(0);
  const [activePowerUp, setActivePowerUp] = useState<string | null>(null);
  const [powerUpDuration, setPowerUpDuration] = useState(0);
  const [nickname, setNickname] = useState("");
  const [highScores, setHighScores] = useState<HighScore[]>([]);
  const [muted, setMuted] = useState(false);

  // References
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const requestRef = useRef<number | null>(null);

  // Keyboard control states
  const keysPressed = useRef<{ [key: string]: boolean }>({});

  // Game Engine Entities
  const playerRef = useRef({
    x: 375,
    y: 530,
    width: 50,
    height: 35,
    speed: 7,
    cooldown: 0,
    maxCooldown: 12,
    shieldDuration: 0,
  });

  const projectilesRef = useRef<Projectile[]>([]);
  const invadersRef = useRef<Invader[]>([]);
  const bossRef = useRef<Boss>({
    x: 300,
    y: 60,
    width: 180,
    height: 65,
    hp: 0,
    maxHp: 120,
    direction: 1,
    laserCooldown: 0,
    active: false,
  });
  const bunkersRef = useRef<BunkerBlock[]>([]);
  const powerUpsRef = useRef<PowerUp[]>([]);
  const particlesRef = useRef<Particle[]>([]);
  const starsRef = useRef<Star[]>([]);

  // Invader Movement Variables
  const invaderMoveDir = useRef(1); // 1 = right, -1 = left
  const invaderMoveTimer = useRef(0);
  const invaderMoveSpeed = useRef(40);
  const invaderStepDown = useRef(false);
  const freezeTimer = useRef(0);
  const screenShake = useRef(0);

  // Load High Scores
  useEffect(() => {
    const loaded = localStorage.getItem("space_neon_highscores");
    if (loaded) {
      try {
        setHighScores(JSON.parse(loaded));
      } catch (e) {
        setHighScores(getDefaultScores());
      }
    } else {
      const defaults = getDefaultScores();
      setHighScores(defaults);
      localStorage.setItem("space_neon_highscores", JSON.stringify(defaults));
    }
  }, []);

  const getDefaultScores = (): HighScore[] => [
    { name: "NEON_COMMANDER", score: 18000, level: 5, date: "2088-08-08" },
    { name: "RETRO_CHAMP", score: 12000, level: 4, date: "2088-08-07" },
    { name: "S_INVADER", score: 8500, level: 3, date: "2088-08-06" },
    { name: "SPACE_BOY", score: 5000, level: 2, date: "2088-08-05" },
    { name: "NOOB", score: 1500, level: 1, date: "2088-08-01" },
  ];

  // Initialize background starfield
  useEffect(() => {
    const stars: Star[] = [];
    for (let i = 0; i < 70; i++) {
      stars.push({
        x: Math.random() * 800,
        y: Math.random() * 600,
        size: Math.random() * 2 + 1,
        speed: Math.random() * 1.5 + 0.5,
      });
    }
    starsRef.current = stars;
  }, []);

  // Set up Keyboard Listeners
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      keysPressed.current[e.code] = true;
      if (e.code === "Space" && gameState === "playing") {
        e.preventDefault();
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      keysPressed.current[e.code] = false;
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);

    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [gameState]);

  // Handle Game Start
  const startGame = () => {
    triggerSound("powerup");
    setScore(0);
    setLevel(1);
    setLives(3);
    setCombo(0);
    setMaxCombo(0);
    setActivePowerUp(null);
    setPowerUpDuration(0);

    // Reset player
    playerRef.current = {
      x: 375,
      y: 530,
      width: 50,
      height: 35,
      speed: 7,
      cooldown: 0,
      maxCooldown: 12,
      shieldDuration: 0,
    };

    initLevel(1);
    setGameState("playing");
  };

  const restartGame = () => {
    startGame();
  };

  // Build Level Wave (5-Stage Campaign)
  const initLevel = (lvl: number) => {
    projectilesRef.current = [];
    powerUpsRef.current = [];
    particlesRef.current = [];
    freezeTimer.current = 0;

    if (lvl === 5) {
      // Level 5 is the Giant Final Boss Mother-ship!
      bossRef.current = {
        x: 310,
        y: 60,
        width: 180,
        height: 65,
        hp: 150,
        maxHp: 150,
        direction: 1,
        laserCooldown: 0,
        active: true,
      };
      invadersRef.current = [];
    } else {
      bossRef.current.active = false;
      const invaders: Invader[] = [];
      const rows = 2 + lvl; // Level 1: 3 rows, Level 2: 4 rows, Level 3: 5 rows, Level 4: 6 rows
      const cols = 9;
      const startX = 100;
      const startY = 80;
      const spacingX = 65;
      const spacingY = 44;

      let id = 0;
      for (let r = 0; r < rows; r++) {
        for (let c = 0; c < cols; c++) {
          // Type configuration for variation
          let type = 1; // regular
          if (r === 0) type = 4; // elite
          else if (r === 1) type = 3; // heavy
          else if (r === 2) type = 2; // swift

          const points = type * 100 + lvl * 25;
          const hasShield = (lvl >= 2 && type === 3) || (lvl >= 3 && type === 4);
          invaders.push({
            id: id++,
            x: startX + c * spacingX,
            y: startY + r * spacingY,
            width: 40,
            height: 30,
            type,
            points,
            hp: hasShield ? 2 : 1,
            maxHp: hasShield ? 2 : 1,
            shield: hasShield,
          });
        }
      }
      invadersRef.current = invaders;
    }

    // Defensive bunkers setup
    initBunkers();

    // Adjust speed based on stage difficulty
    invaderMoveSpeed.current = Math.max(50 - lvl * 7, 12);
    invaderMoveTimer.current = 0;
    invaderMoveDir.current = 1;
  };

  const initBunkers = () => {
    const bunkers: BunkerBlock[] = [];
    const bunkerPositions = [120, 300, 480, 660];
    const blockWidth = 10;
    const blockHeight = 8;

    bunkerPositions.forEach((bx) => {
      // Build classic inverted U protective barriers
      for (let row = 0; row < 4; row++) {
        for (let col = 0; col < 6; col++) {
          if (row === 3 && col >= 2 && col <= 3) continue;
          bunkers.push({
            x: bx + col * blockWidth,
            y: 440 + row * blockHeight,
            width: blockWidth,
            height: blockHeight,
            hp: 4,
          });
        }
      }
    });

    bunkersRef.current = bunkers;
  };

  const toggleMute = () => {
    setMuted(!muted);
  };

  const triggerSound = (type: "laser" | "explosion" | "powerup" | "hit" | "bossLaser" | "gameover" | "shield") => {
    if (!muted) {
      playSound(type);
    }
  };

  // Glowy particle blasts
  const createExplosion = (x: number, y: number, color: string, count = 12) => {
    const newParticles: Particle[] = [];
    for (let i = 0; i < count; i++) {
      const angle = Math.random() * Math.PI * 2;
      const speed = Math.random() * 4.5 + 1;
      newParticles.push({
        x,
        y,
        vx: Math.cos(angle) * speed,
        vy: Math.sin(angle) * speed,
        color,
        size: Math.random() * 3 + 1.5,
        alpha: 1,
        decay: Math.random() * 0.03 + 0.015,
      });
    }
    particlesRef.current = [...particlesRef.current, ...newParticles];
  };

  const submitScore = (e: React.FormEvent) => {
    e.preventDefault();
    if (!nickname.trim()) return;

    const newScore: HighScore = {
      name: nickname.toUpperCase().substring(0, 15),
      score,
      level,
      date: new Date().toISOString().split("T")[0],
    };

    const updated = [...highScores, newScore]
      .sort((a, b) => b.score - a.score)
      .slice(0, 8);

    setHighScores(updated);
    localStorage.setItem("space_neon_highscores", JSON.stringify(updated));
    setNickname("");
    setGameState("start");
  };

  // Main Canvas Game Loop
  useEffect(() => {
    if (gameState !== "playing") {
      if (requestRef.current) cancelAnimationFrame(requestRef.current);
      return;
    }

    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    let localPowerUp = activePowerUp;
    let localPowerUpDuration = powerUpDuration;

    const updateGame = () => {
      // 1. Stars Parallax Movement
      starsRef.current.forEach((star) => {
        star.y += star.speed;
        if (star.y > 600) {
          star.y = 0;
          star.x = Math.random() * 800;
        }
      });

      // 2. Player Ship Controls
      const player = playerRef.current;
      if (keysPressed.current["ArrowLeft"] || keysPressed.current["KeyA"]) {
        player.x = Math.max(player.x - player.speed, 10);
      }
      if (keysPressed.current["ArrowRight"] || keysPressed.current["KeyD"]) {
        player.x = Math.min(player.x + player.speed, 800 - player.width - 10);
      }

      // Check shield durations
      if (player.shieldDuration > 0) {
        player.shieldDuration--;
        if (player.shieldDuration === 0) {
          setActivePowerUp(null);
          localPowerUp = null;
        }
      }

      // Check weapon upgrade duration
      if (localPowerUpDuration > 0 && localPowerUp !== "shield") {
        localPowerUpDuration--;
        setPowerUpDuration(localPowerUpDuration);
        if (localPowerUpDuration === 0) {
          setActivePowerUp(null);
          localPowerUp = null;
        }
      }

      // Tick weapon fire cooldown
      if (player.cooldown > 0) player.cooldown--;

      // Spacebar active shoot
      if (keysPressed.current["Space"] && player.cooldown === 0) {
        firePlayerLaser();
      }

      // 3. Move Projectiles
      const projectiles = projectilesRef.current;
      projectilesRef.current = projectiles
        .map((p) => {
          p.y += p.dy;
          return p;
        })
        .filter((p) => p.y > -20 && p.y < 620);

      // 4. Chrono Freeze Check
      if (freezeTimer.current > 0) {
        freezeTimer.current--;
      }

      // 5. Invaders Squad logic
      const invaders = invadersRef.current;
      const isFrozen = freezeTimer.current > 0;

      if (bossRef.current.active) {
        // Special Boss Action Sequence
        const boss = bossRef.current;
        boss.x += boss.direction * (isFrozen ? 0.6 : 2.2);
        if (boss.x <= 30 || boss.x >= 800 - boss.width - 30) {
          boss.direction *= -1;
        }

        boss.laserCooldown--;
        if (boss.laserCooldown <= 0) {
          // Double direct side lasers
          projectilesRef.current.push({
            x: boss.x + boss.width * 0.15,
            y: boss.y + boss.height,
            width: 4,
            height: 15,
            dy: 4.8,
            isPlayer: false,
            type: "standard",
          });
          projectilesRef.current.push({
            x: boss.x + boss.width * 0.85,
            y: boss.y + boss.height,
            width: 4,
            height: 15,
            dy: 4.8,
            isPlayer: false,
            type: "standard",
          });

          // Special center beam attack
          if (Math.random() < 0.4) {
            triggerSound("bossLaser");
            projectilesRef.current.push({
              x: boss.x + boss.width * 0.5 - 4,
              y: boss.y + boss.height,
              width: 8,
              height: 25,
              dy: 6.0,
              isPlayer: false,
              type: "laser-beam",
            });
          }
          boss.laserCooldown = Math.max(38 - level * 4, 18);
        }
      } else if (invaders.length > 0) {
        // Standard invaders squad pacing
        invaderMoveTimer.current++;
        const currentSpeed = isFrozen ? invaderMoveSpeed.current * 3.5 : invaderMoveSpeed.current;
        if (invaderMoveTimer.current >= currentSpeed) {
          invaderMoveTimer.current = 0;

          // Boundary bounce detection
          let hitSide = false;
          invaders.forEach((inv) => {
            const nextX = inv.x + invaderMoveDir.current * 14;
            if (nextX <= 15 || nextX >= 800 - inv.width - 15) {
              hitSide = true;
            }
          });

          if (hitSide) {
            invaderMoveDir.current *= -1;
            invaderStepDown.current = true;
          }

          invaders.forEach((inv) => {
            if (invaderStepDown.current) {
              inv.y += 24;
              // Check invasion breach fail condition
              if (inv.y + inv.height >= 460) {
                triggerSound("gameover");
                setGameState("gameover");
              }
            } else {
              inv.x += invaderMoveDir.current * 14;
            }
          });
          invaderStepDown.current = false;
        }

        // Random invader bomb drop pacing
        const fireChance = Math.max(0.008 + level * 0.003, 0.028);
        if (Math.random() < fireChance && invaders.length > 0) {
          const livingInvaders = invaders.filter((i) => i.hp > 0);
          if (livingInvaders.length > 0) {
            const randomInv = livingInvaders[Math.floor(Math.random() * livingInvaders.length)];
            projectilesRef.current.push({
              x: randomInv.x + randomInv.width / 2,
              y: randomInv.y + randomInv.height,
              width: 4,
              height: 12,
              dy: Math.min(3.6 + level * 0.4, 7.2),
              isPlayer: false,
              type: "standard",
            });
          }
        }
      } else {
        // Current Wave Cleared!
        if (level === 5) {
          // Campaign Victory Condition reached!
          triggerSound("powerup");
          setGameState("campaign_clear");
        } else {
          setGameState("victory");
          setTimeout(() => {
            setLevel((prev) => {
              const nextLvl = prev + 1;
              initLevel(nextLvl);
              setGameState("playing");
              return nextLvl;
            });
          }, 1800);
        }
      }

      // 6. Powerups Movement
      powerUpsRef.current = powerUpsRef.current
        .map((p) => {
          p.y += p.dy;
          return p;
        })
        .filter((p) => p.y < 610);

      // 7. Render/Pacing of debris particles
      particlesRef.current = particlesRef.current
        .map((p) => {
          p.x += p.vx;
          p.y += p.vy;
          p.alpha -= p.decay;
          return p;
        })
        .filter((p) => p.alpha > 0);

      // 8. Run collisions check
      handleCollisions();

      // 9. Decay screenshake
      if (screenShake.current > 0) {
        screenShake.current -= 0.6;
        if (screenShake.current < 0) screenShake.current = 0;
      }

      // Render updated frame
      renderCanvas(ctx, player);

      requestRef.current = requestAnimationFrame(updateGame);
    };

    const firePlayerLaser = () => {
      const player = playerRef.current;
      triggerSound("laser");

      if (localPowerUp === "triple") {
        // Triple spread lasers
        projectilesRef.current.push({
          x: player.x + player.width / 2 - 2,
          y: player.y,
          width: 4,
          height: 14,
          dy: -9,
          isPlayer: true,
          type: "triple",
        });
        projectilesRef.current.push({
          x: player.x + 5,
          y: player.y + 10,
          width: 4,
          height: 14,
          dy: -8.5,
          isPlayer: true,
          type: "triple",
        });
        projectilesRef.current.push({
          x: player.x + player.width - 9,
          y: player.y + 10,
          width: 4,
          height: 14,
          dy: -8.5,
          isPlayer: true,
          type: "triple",
        });
        player.cooldown = player.maxCooldown + 2;
      } else if (localPowerUp === "laser-beam") {
        // Dense glowing rapid green beam
        projectilesRef.current.push({
          x: player.x + player.width / 2 - 4,
          y: player.y - 10,
          width: 8,
          height: 25,
          dy: -12.5,
          isPlayer: true,
          type: "laser-beam",
        });
        player.cooldown = Math.max(player.maxCooldown - 7, 3);
      } else {
        // Standard single blue-cyan laser
        projectilesRef.current.push({
          x: player.x + player.width / 2 - 2,
          y: player.y,
          width: 4,
          height: 14,
          dy: -8.5,
          isPlayer: true,
          type: "standard",
        });
        player.cooldown = player.maxCooldown;
      }
    };

    const handleCollisions = () => {
      const player = playerRef.current;
      const projectiles = projectilesRef.current;
      const invaders = invadersRef.current;
      const bunkers = bunkersRef.current;
      const powerUps = powerUpsRef.current;

      // Iterate backwards to avoid index shifting bugs
      for (let pIndex = projectiles.length - 1; pIndex >= 0; pIndex--) {
        const p = projectiles[pIndex];

        // A. Friendly Laser colliding with Boss/Invaders
        if (p.isPlayer) {
          if (bossRef.current.active) {
            const boss = bossRef.current;
            if (
              p.x > boss.x &&
              p.x < boss.x + boss.width &&
              p.y > boss.y &&
              p.y < boss.y + boss.height
            ) {
              projectiles.splice(pIndex, 1);
              triggerSound("hit");
              boss.hp -= p.type === "laser-beam" ? 3 : 1;
              createExplosion(p.x, p.y, "#ff0077", 6);
              screenShake.current = 4;

              if (boss.hp <= 0) {
                boss.active = false;
                triggerSound("explosion");
                setScore((s) => s + 5000);
                createExplosion(boss.x + boss.width / 2, boss.y + boss.height / 2, "#ff00aa", 40);
                setGameState("campaign_clear");
              }
              continue;
            }
          }

          for (let iIndex = invaders.length - 1; iIndex >= 0; iIndex--) {
            const inv = invaders[iIndex];
            if (
              p.x > inv.x &&
              p.x < inv.x + inv.width &&
              p.y > inv.y &&
              p.y < inv.y + inv.height
            ) {
              projectiles.splice(pIndex, 1);
              inv.hp -= p.type === "laser-beam" ? 2 : 1;

              if (inv.hp <= 0) {
                invaders.splice(iIndex, 1);
                triggerSound("explosion");
                createExplosion(inv.x + inv.width / 2, inv.y + inv.height / 2, getInvaderColor(inv.type), 15);
                screenShake.current = 6;

                // Score + combo update
                setCombo((c) => {
                  const nextC = c + 1;
                  setMaxCombo((m) => Math.max(m, nextC));
                  const basePoints = inv.points;
                  const multiplier = 1 + Math.floor(nextC / 5) * 0.2;
                  setScore((s) => s + Math.round(basePoints * multiplier));
                  return nextC;
                });

                // Spawn powerup chance
                if (Math.random() < 0.18) {
                  spawnPowerUp(inv.x + inv.width / 2, inv.y);
                }
              } else {
                triggerSound("hit");
                createExplosion(p.x, p.y, "#00ffcc", 4);
              }
              break;
            }
          }
        }

        // B. Enemy Projectiles hitting Hero Ship
        if (!p.isPlayer) {
          if (
            p.x > player.x &&
            p.x < player.x + player.width &&
            p.y > player.y &&
            p.y < player.y + player.height
          ) {
            projectiles.splice(pIndex, 1);
            screenShake.current = 16;

            if (player.shieldDuration > 0) {
              triggerSound("shield");
              createExplosion(p.x, p.y, "#00e5ff", 10);
            } else {
              triggerSound("explosion");
              createExplosion(player.x + player.width / 2, player.y + player.height / 2, "#ff3300", 25);
              setCombo(0);
              setLives((l) => {
                const nextL = l - 1;
                if (nextL <= 0) {
                  setGameState("gameover");
                }
                return nextL;
              });
            }
            continue;
          }
        }

        // C. Laser Collisions with destructible bunkers
        for (let bIndex = bunkers.length - 1; bIndex >= 0; bIndex--) {
          const b = bunkers[bIndex];
          if (
            p.x > b.x &&
            p.x < b.x + b.width &&
            p.y > b.y &&
            p.y < b.y + b.height
          ) {
            projectiles.splice(pIndex, 1);
            triggerSound("hit");
            createExplosion(p.x, p.y, "#e0f2fe", 4);
            b.hp -= 1;
            if (b.hp <= 0) {
              bunkers.splice(bIndex, 1);
            }
            break;
          }
        }
      }

      // D. Powerup pickup collision
      for (let puIndex = powerUps.length - 1; puIndex >= 0; puIndex--) {
        const pu = powerUps[puIndex];
        if (
          pu.x > player.x &&
          pu.x < player.x + player.width &&
          pu.y > player.y &&
          pu.y < player.y + player.height
        ) {
          powerUps.splice(puIndex, 1);
          triggerSound("powerup");
          setActivePowerUp(pu.type);

          if (pu.type === "shield") {
            player.shieldDuration = 400; // frames
            setPowerUpDuration(400);
          } else if (pu.type === "freeze") {
            freezeTimer.current = 240;
            setPowerUpDuration(240);
          } else if (pu.type === "bomb") {
            triggerSound("explosion");
            screenShake.current = 20;
            if (bossRef.current.active) {
              bossRef.current.hp = Math.max(1, bossRef.current.hp - 20);
              createExplosion(bossRef.current.x + bossRef.current.width / 2, bossRef.current.y + bossRef.current.height / 2, "#facc15", 30);
            } else {
              const count = Math.ceil(invaders.length / 2);
              for (let i = 0; i < count; i++) {
                if (invaders.length > 0) {
                  const targetIdx = Math.floor(Math.random() * invaders.length);
                  const target = invaders[targetIdx];
                  createExplosion(target.x + target.width / 2, target.y + target.height / 2, "#facc15", 12);
                  setScore((s) => s + target.points);
                  invaders.splice(targetIdx, 1);
                }
              }
            }
            setActivePowerUp(null);
          } else {
            // Weapon upgrades
            localPowerUp = pu.type;
            localPowerUpDuration = 350;
            setPowerUpDuration(350);
          }
        }
      }
    };

    const spawnPowerUp = (x: number, y: number) => {
      const types: Array<"triple" | "laser-beam" | "shield" | "freeze" | "bomb"> = [
        "triple",
        "laser-beam",
        "shield",
        "freeze",
        "bomb",
      ];
      const type = types[Math.floor(Math.random() * types.length)];
      powerUpsRef.current.push({
        x,
        y,
        width: 24,
        height: 24,
        type,
        dy: 2.2,
      });
    };

    // Rendering core
    const renderCanvas = (ctx: CanvasRenderingContext2D, player: typeof playerRef.current) => {
      ctx.clearRect(0, 0, 800, 600);

      ctx.save();
      if (screenShake.current > 0) {
        const dx = (Math.random() - 0.5) * screenShake.current;
        const dy = (Math.random() - 0.5) * screenShake.current;
        ctx.translate(dx, dy);
      }

      // Draw Starfield
      ctx.fillStyle = "rgba(255, 255, 255, 0.35)";
      starsRef.current.forEach((star) => {
        ctx.fillRect(star.x, star.y, star.size, star.size);
      });

      // Draw bunkers
      bunkersRef.current.forEach((b) => {
        if (b.hp === 4) ctx.fillStyle = "#38bdf8";
        else if (b.hp === 3) ctx.fillStyle = "#0284c7";
        else if (b.hp === 2) ctx.fillStyle = "#0369a1";
        else ctx.fillStyle = "#0c4a6e";
        ctx.fillRect(b.x, b.y, b.width, b.height);
      });

      // Draw Invaders
      invadersRef.current.forEach((inv) => {
        ctx.fillStyle = getInvaderColor(inv.type);
        drawRetroInvader(ctx, inv);

        if (inv.shield && inv.hp > 1) {
          ctx.strokeStyle = "#00ffff";
          ctx.lineWidth = 1.5;
          ctx.strokeRect(inv.x - 2, inv.y - 2, inv.width + 4, inv.height + 4);
        }
      });

      // Draw Mothership Boss
      if (bossRef.current.active) {
        drawMothership(ctx, bossRef.current);
      }

      // Draw Projectiles
      projectilesRef.current.forEach((p) => {
        if (p.isPlayer) {
          if (p.type === "laser-beam") {
            ctx.fillStyle = "#10b981";
            ctx.shadowColor = "#10b981";
            ctx.shadowBlur = 10;
          } else {
            ctx.fillStyle = "#00f5ff";
            ctx.shadowColor = "#00f5ff";
            ctx.shadowBlur = 6;
          }
        } else {
          ctx.fillStyle = "#ff0077";
          ctx.shadowColor = "#ff0077";
          ctx.shadowBlur = 8;
        }
        ctx.fillRect(p.x, p.y, p.width, p.height);
        ctx.shadowBlur = 0;
      });

      // Draw Powerups
      powerUpsRef.current.forEach((pu) => {
        ctx.save();
        ctx.fillStyle = "#00ffcc";
        ctx.shadowColor = "#00ffcc";
        ctx.shadowBlur = 12;

        ctx.beginPath();
        ctx.moveTo(pu.x + pu.width / 2, pu.y);
        ctx.lineTo(pu.x + pu.width, pu.y + pu.height / 2);
        ctx.lineTo(pu.x + pu.width / 2, pu.y + pu.height);
        ctx.lineTo(pu.x, pu.y + pu.height / 2);
        ctx.closePath();
        ctx.fill();

        ctx.fillStyle = "#000000";
        ctx.font = "bold 10px monospace";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        const symbol = pu.type.substring(0, 1).toUpperCase();
        ctx.fillText(symbol, pu.x + pu.width / 2, pu.y + pu.height / 2);
        ctx.restore();
      });

      // Draw Explosion Particles
      particlesRef.current.forEach((p) => {
        ctx.fillStyle = p.color;
        ctx.globalAlpha = p.alpha;
        ctx.fillRect(p.x, p.y, p.size, p.size);
      });
      ctx.globalAlpha = 1.0;

      // Draw Player Hero Ship
      drawSpaceShip(ctx, player);

      ctx.restore();

      if (freezeTimer.current > 0) {
        ctx.fillStyle = "rgba(0, 245, 255, 0.08)";
        ctx.fillRect(0, 0, 800, 600);
        ctx.strokeStyle = "rgba(0, 245, 255, 0.4)";
        ctx.lineWidth = 4;
        ctx.strokeRect(0, 0, 800, 600);
      }
    };

    const getInvaderColor = (type: number) => {
      if (type === 4) return "#f43f5e"; // Elite heavy pink
      if (type === 3) return "#d946ef"; // Armor magenta
      if (type === 2) return "#06b6d4"; // Cyan swift
      return "#84cc16"; // Lime regular
    };

    const drawRetroInvader = (ctx: CanvasRenderingContext2D, inv: Invader) => {
      const w = inv.width;
      const h = inv.height;
      const x = inv.x;
      const y = inv.y;

      ctx.save();
      ctx.fillStyle = getInvaderColor(inv.type);

      if (inv.type === 4) {
        // High Tier Skull Elite
        ctx.fillRect(x + w * 0.25, y, w * 0.5, h * 0.2);
        ctx.fillRect(x + w * 0.1, y + h * 0.2, w * 0.8, h * 0.2);
        ctx.fillRect(x, y + h * 0.4, w, h * 0.2);
        ctx.fillRect(x + w * 0.15, y + h * 0.6, w * 0.2, h * 0.2);
        ctx.fillRect(x + w * 0.65, y + h * 0.6, w * 0.2, h * 0.2);
        ctx.fillRect(x, y + h * 0.8, w * 0.15, h * 0.2);
        ctx.fillRect(x + w * 0.85, y + h * 0.8, w * 0.15, h * 0.2);
      } else if (inv.type === 3) {
        // Crab Shielded invader
        ctx.fillRect(x + w * 0.1, y, w * 0.2, h * 0.2);
        ctx.fillRect(x + w * 0.7, y, w * 0.2, h * 0.2);
        ctx.fillRect(x + w * 0.2, y + h * 0.2, w * 0.6, h * 0.2);
        ctx.fillRect(x, y + h * 0.4, w, h * 0.2);
        ctx.fillRect(x + w * 0.2, y + h * 0.6, w * 0.15, h * 0.2);
        ctx.fillRect(x + w * 0.65, y + h * 0.6, w * 0.15, h * 0.2);
        ctx.fillRect(x + w * 0.05, y + h * 0.8, w * 0.15, h * 0.2);
        ctx.fillRect(x + w * 0.8, y + h * 0.8, w * 0.15, h * 0.2);
      } else if (inv.type === 2) {
        // Swift Beetle
        ctx.fillRect(x + w * 0.3, y, w * 0.4, h * 0.2);
        ctx.fillRect(x + w * 0.15, y + h * 0.2, w * 0.7, h * 0.2);
        ctx.fillRect(x, y + h * 0.4, w, h * 0.2);
        ctx.fillRect(x + w * 0.2, y + h * 0.6, w * 0.6, h * 0.2);
        ctx.fillRect(x + w * 0.1, y + h * 0.8, w * 0.15, h * 0.2);
        ctx.fillRect(x + w * 0.75, y + h * 0.8, w * 0.15, h * 0.2);
      } else {
        // Regular insect squid
        ctx.fillRect(x + w * 0.35, y, w * 0.3, h * 0.2);
        ctx.fillRect(x + w * 0.2, y + h * 0.2, w * 0.6, h * 0.2);
        ctx.fillRect(x + w * 0.1, y + h * 0.4, w * 0.8, h * 0.2);
        ctx.fillRect(x + w * 0.25, y + h * 0.6, w * 0.5, h * 0.2);
        ctx.fillRect(x + w * 0.1, y + h * 0.8, w * 0.2, h * 0.2);
        ctx.fillRect(x + w * 0.7, y + h * 0.8, w * 0.2, h * 0.2);
      }

      ctx.restore();
    };

    const drawMothership = (ctx: CanvasRenderingContext2D, boss: Boss) => {
      const { x, y, width, height } = boss;

      ctx.save();
      ctx.shadowColor = "#ff0077";
      ctx.shadowBlur = 15;
      ctx.fillStyle = "#330022";
      ctx.strokeStyle = "#ff0077";
      ctx.lineWidth = 3;

      // Draw complex Mothership UFO
      ctx.beginPath();
      ctx.moveTo(x + width * 0.3, y);
      ctx.lineTo(x + width * 0.7, y);
      ctx.lineTo(x + width * 0.85, y + height * 0.4);
      ctx.lineTo(x + width, y + height * 0.6);
      ctx.lineTo(x + width * 0.85, y + height * 0.8);
      ctx.lineTo(x + width * 0.15, y + height * 0.8);
      ctx.lineTo(x, y + height * 0.6);
      ctx.lineTo(x + width * 0.15, y + height * 0.4);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();

      // Yellow core shield lights
      ctx.fillStyle = "#facc15";
      ctx.shadowColor = "#facc15";
      ctx.shadowBlur = 8;
      for (let i = 1; i <= 6; i++) {
        const lx = x + width * (i * 0.14);
        const ly = y + height * 0.6;
        ctx.beginPath();
        ctx.arc(lx, ly, 4.5, 0, Math.PI * 2);
        ctx.fill();
      }

      // Boss Life Bar Rendering
      ctx.restore();
      ctx.fillStyle = "rgba(0,0,0,0.6)";
      ctx.fillRect(200, 15, 400, 15);
      ctx.strokeStyle = "#ff0077";
      ctx.strokeRect(200, 15, 400, 15);
      const hpPercent = boss.hp / boss.maxHp;
      ctx.fillStyle = hpPercent > 0.4 ? "#ff0055" : "#ff0000";
      ctx.fillRect(201, 16, 398 * hpPercent, 13);

      ctx.fillStyle = "#ffffff";
      ctx.font = "10px monospace";
      ctx.textAlign = "center";
      ctx.fillText("ALIENT MOTHER-SHIP CLASS BOSS", 400, 11);
    };

    const drawSpaceShip = (ctx: CanvasRenderingContext2D, p: typeof playerRef.current) => {
      ctx.save();

      ctx.shadowColor = "#00f5ff";
      ctx.shadowBlur = playerRef.current.shieldDuration > 0 ? 15 : 6;
      ctx.fillStyle = "#0c2340";
      ctx.strokeStyle = playerRef.current.shieldDuration > 0 ? "#00ffff" : "#00f5ff";
      ctx.lineWidth = 2.5;

      // Ship geometry
      ctx.beginPath();
      ctx.moveTo(p.x + p.width / 2, p.y);
      ctx.lineTo(p.x + p.width, p.y + p.height * 0.85);
      ctx.lineTo(p.x + p.width * 0.8, p.y + p.height);
      ctx.lineTo(p.x + p.width * 0.2, p.y + p.height);
      ctx.lineTo(p.x, p.y + p.height * 0.85);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();

      // Thruster Fire
      if (Math.random() > 0.3) {
        ctx.fillStyle = "#f97316";
        ctx.shadowColor = "#ef4444";
        ctx.shadowBlur = 8;
        ctx.beginPath();
        ctx.moveTo(p.x + p.width * 0.4, p.y + p.height);
        ctx.lineTo(p.x + p.width / 2, p.y + p.height + (Math.random() * 10 + 4));
        ctx.lineTo(p.x + p.width * 0.6, p.y + p.height);
        ctx.closePath();
        ctx.fill();
      }

      // Shield sphere effect
      if (p.shieldDuration > 0) {
        ctx.strokeStyle = `rgba(0, 245, 255, ${0.4 + Math.sin(Date.now() / 80) * 0.3})`;
        ctx.lineWidth = 3;
        ctx.beginPath();
        ctx.arc(p.x + p.width / 2, p.y + p.height / 2, p.width * 0.85, 0, Math.PI * 2);
        ctx.stroke();
      }

      ctx.restore();
    };

    requestRef.current = requestAnimationFrame(updateGame);

    return () => {
      if (requestRef.current) cancelAnimationFrame(requestRef.current);
    };
  }, [gameState, activePowerUp, powerUpDuration, level, muted]);

  // Mobile/Mouse Buttons controls
  const handleMobileLeftStart = () => { keysPressed.current["ArrowLeft"] = true; };
  const handleMobileLeftEnd = () => { keysPressed.current["ArrowLeft"] = false; };
  const handleMobileRightStart = () => { keysPressed.current["ArrowRight"] = true; };
  const handleMobileRightEnd = () => { keysPressed.current["ArrowRight"] = false; };
  const handleMobileShoot = () => {
    const player = playerRef.current;
    if (gameState === "playing" && player.cooldown === 0) {
      triggerSound("laser");
      const localPowerUp = activePowerUp;
      if (localPowerUp === "triple") {
        projectilesRef.current.push({
          x: player.x + player.width / 2 - 2,
          y: player.y,
          width: 4,
          height: 14,
          dy: -9,
          isPlayer: true,
          type: "triple",
        });
        projectilesRef.current.push({
          x: player.x + 5,
          y: player.y + 10,
          width: 4,
          height: 14,
          dy: -8.5,
          isPlayer: true,
          type: "triple",
        });
        projectilesRef.current.push({
          x: player.x + player.width - 9,
          y: player.y + 10,
          width: 4,
          height: 14,
          dy: -8.5,
          isPlayer: true,
          type: "triple",
        });
        player.cooldown = player.maxCooldown + 2;
      } else if (localPowerUp === "laser-beam") {
        projectilesRef.current.push({
          x: player.x + player.width / 2 - 4,
          y: player.y - 10,
          width: 8,
          height: 25,
          dy: -12.5,
          isPlayer: true,
          type: "laser-beam",
        });
        player.cooldown = Math.max(player.maxCooldown - 7, 3);
      } else {
        projectilesRef.current.push({
          x: player.x + player.width / 2 - 2,
          y: player.y,
          width: 4,
          height: 14,
          dy: -8.5,
          isPlayer: true,
          type: "standard",
        });
        player.cooldown = player.maxCooldown;
      }
    }
  };

  const anvilStateJSON = JSON.stringify({
    score,
    level,
    lives,
    gameState,
    powerUp: activePowerUp,
    combo,
    highScores,
  });

  return (
    <main className="min-h-screen crt-effect scanline flex flex-col items-center justify-between p-4 md:p-8 select-none">
      <div id="anvil-state" data-anvil-state={anvilStateJSON} className="hidden" />

      <header className="w-full max-w-4xl flex flex-col sm:flex-row items-center justify-between border-b border-pink-500/30 pb-4 mb-2">
        <div>
          <h1 className="text-3xl md:text-5xl font-extrabold tracking-widest text-center sm:text-left bg-gradient-to-r from-pink-500 via-purple-500 to-cyan-500 bg-clip-text text-transparent drop-shadow-[0_0_12px_rgba(236,72,153,0.5)]">
            NEON SHIELD
          </h1>
          <p className="text-xs font-mono text-cyan-400 text-center sm:text-left tracking-wider">
            INVADERS RETRO SCI-FI ARCADE
          </p>
        </div>

        <div className="flex items-center gap-4 mt-4 sm:mt-0">
          <button
            onClick={toggleMute}
            className="px-3 py-1 bg-slate-900 border border-slate-700 rounded text-xs font-mono text-slate-400 hover:text-white hover:border-cyan-400 transition-colors"
          >
            {muted ? "🔇 SOUND OFF" : "🔊 SOUND ON"}
          </button>
          <div className="text-right font-mono text-sm hidden sm:block">
            <span className="text-pink-500">HI-SCORE: </span>
            <span className="text-white font-bold">
              {Math.max(score, highScores[0]?.score || 0)}
            </span>
          </div>
        </div>
      </header>

      <div className="w-full max-w-4xl flex flex-col lg:flex-row gap-6 items-stretch justify-center">
        <div className="flex-1 flex flex-col items-center bg-slate-950/90 rounded-xl border border-cyan-500/30 shadow-[0_0_25px_rgba(6,182,212,0.15)] relative overflow-hidden">
          {gameState === "playing" && (
            <div className="w-full flex items-center justify-between px-6 py-3 border-b border-cyan-500/20 bg-slate-900/40 text-sm font-mono z-10">
              <div className="flex gap-6">
                <div>
                  <span className="text-slate-400">SCORE </span>
                  <span className="text-yellow-400 font-bold text-lg">{score}</span>
                </div>
                <div>
                  <span className="text-slate-400">WAVE </span>
                  <span className="text-cyan-400 font-bold text-lg">{level}/5</span>
                </div>
              </div>

              {activePowerUp && (
                <div className="px-3 py-0.5 bg-cyan-950 border border-cyan-400 text-cyan-300 rounded text-xs animate-pulse">
                  POWER: <span className="font-bold">{activePowerUp.toUpperCase()}</span>
                </div>
              )}

              <div className="flex items-center gap-4">
                {combo > 1 && (
                  <div className="text-pink-500 animate-bounce">
                    x{combo} <span className="text-xs text-slate-400">COMBO</span>
                  </div>
                )}
                <div className="flex gap-1.5">
                  {Array.from({ length: 3 }).map((_, i) => (
                    <div
                      key={i}
                      className={`w-4 h-5 border-2 rounded-t-lg transition-all ${
                        i < lives
                          ? "bg-cyan-500 border-cyan-400 shadow-[0_0_6px_#22d3ee]"
                          : "bg-transparent border-slate-800"
                      }`}
                    />
                  ))}
                </div>
              </div>
            </div>
          )}

          <div className="relative">
            <canvas
              ref={canvasRef}
              width={800}
              height={600}
              className="max-w-full aspect-[4/3] bg-black block cursor-crosshair"
            />

            {gameState === "start" && (
              <div className="absolute inset-0 bg-slate-950/95 flex flex-col items-center justify-center p-6 text-center">
                <div className="space-y-6 max-w-md">
                  <div className="space-y-1">
                    <span className="text-pink-500 text-xs font-mono uppercase tracking-widest">
                      🌌 Cybernetic Conflict Initiated 🌌
                    </span>
                    <h2 className="text-4xl font-extrabold tracking-tight text-white drop-shadow-[0_0_10px_rgba(255,0,255,0.6)]">
                      START OPERATION
                    </h2>
                  </div>

                  <p className="text-slate-400 text-sm font-mono leading-relaxed">
                    Protect the neon energy grid from 5 waves of incoming space invaders. Shoot down core squads to discover powerups and weapons. Level 5 holds the giant Mothership Boss!
                  </p>

                  <div className="bg-slate-900/60 p-4 rounded-lg border border-slate-800 text-left space-y-2 text-xs font-mono">
                    <div className="text-center text-cyan-400 font-bold border-b border-slate-800 pb-1 mb-1">
                      🕹️ KEYBOARD / TOUCH CONTROLS
                    </div>
                    <div className="flex justify-between">
                      <span className="text-slate-400">Move Wing:</span>
                      <span className="text-white">A / D or ⬅️ / ➡️ Arrows</span>
                    </div>
                    <div className="flex justify-between">
                      <span className="text-slate-400">Fire Weapon:</span>
                      <span className="text-white">Spacebar or Tap On-Screen</span>
                    </div>
                  </div>

                  <button
                    onClick={startGame}
                    data-anvil-action="primary"
                    className="w-full py-4 bg-gradient-to-r from-cyan-500 to-pink-500 hover:from-cyan-400 hover:to-pink-400 text-white font-mono font-bold uppercase tracking-widest rounded-lg shadow-[0_0_20px_rgba(6,182,212,0.4)] hover:shadow-[0_0_25px_rgba(236,72,153,0.6)] transition-all transform active:scale-95"
                  >
                    🚀 ENGAGE INVADERS 🚀
                  </button>
                </div>
              </div>
            )}

            {gameState === "victory" && (
              <div className="absolute inset-0 bg-black/85 flex flex-col items-center justify-center p-6 text-center">
                <div className="space-y-4">
                  <div className="inline-block p-4 bg-cyan-950/50 border-2 border-cyan-400 rounded-full animate-bounce">
                    🪐
                  </div>
                  <h2 className="text-4xl md:text-5xl font-extrabold tracking-widest text-cyan-400 drop-shadow-[0_0_15px_rgba(34,211,238,0.7)]">
                    WAVE CLEARED!
                  </h2>
                  <p className="text-slate-300 font-mono text-sm max-w-sm">
                    Threat neutralized. Repairing wing components and charging hyperdrive...
                  </p>
                  <p className="text-yellow-400 font-mono font-bold text-lg animate-pulse">
                    PREPARING WAVE {level + 1}/5
                  </p>
                </div>
              </div>
            )}

            {gameState === "campaign_clear" && (
              <div className="absolute inset-0 bg-slate-950/95 flex flex-col items-center justify-center p-6 text-center">
                <div className="space-y-5 max-w-md w-full">
                  <div className="inline-block p-4 bg-yellow-950/50 border-2 border-yellow-400 rounded-full animate-bounce">
                    👑
                  </div>
                  <h2 className="text-4xl font-extrabold text-transparent bg-gradient-to-r from-yellow-400 via-orange-500 to-pink-500 bg-clip-text drop-shadow-[0_0_15px_rgba(234,179,8,0.5)]">
                    CAMPAIGN CLEARED!
                  </h2>
                  <p className="text-slate-300 font-mono text-sm">
                    You defeated the Giant Alien Mothership and saved the neon grid! You are an elite pilot!
                  </p>
                  <div className="space-y-1 font-mono">
                    <p className="text-slate-400 text-xs">FINAL COMBAT SCORE</p>
                    <p className="text-4xl font-extrabold text-yellow-400">{score}</p>
                  </div>

                  <form
                    onSubmit={submitScore}
                    className="bg-slate-900/80 p-5 rounded-lg border border-yellow-500/20 space-y-3"
                  >
                    <label className="block text-xs text-yellow-400 font-mono uppercase tracking-wider font-semibold">
                      Enter High Score Leaderboard
                    </label>
                    <div className="flex gap-2">
                      <input
                        type="text"
                        maxLength={14}
                        placeholder="PILOT ALIAS"
                        value={nickname}
                        onChange={(e) => setNickname(e.target.value)}
                        data-anvil-action="input"
                        className="flex-1 bg-black border border-slate-700 rounded px-3 py-2 text-white font-mono placeholder-slate-600 focus:outline-none focus:border-yellow-400 text-center uppercase tracking-wider text-sm"
                        required
                      />
                      <button
                        type="submit"
                        className="px-5 py-2 bg-yellow-600 hover:bg-yellow-500 font-mono text-xs font-bold text-white uppercase tracking-wider rounded transition-colors"
                      >
                        SUBMIT
                      </button>
                    </div>
                  </form>

                  <button
                    onClick={restartGame}
                    data-anvil-action="restart"
                    className="w-full py-3 bg-slate-900 hover:bg-slate-800 border border-slate-700 text-white font-mono text-xs font-bold uppercase tracking-widest rounded transition-colors"
                  >
                    🔄 PLAY AGAIN
                  </button>
                </div>
              </div>
            )}

            {gameState === "gameover" && (
              <div className="absolute inset-0 bg-slate-950/95 flex flex-col items-center justify-center p-6 text-center">
                <div className="space-y-5 max-w-md w-full">
                  <h2 className="text-4xl font-black text-red-500 tracking-wider drop-shadow-[0_0_12px_rgba(239,68,68,0.6)] animate-pulse">
                    WING DESTROYED
                  </h2>
                  <div className="space-y-1 font-mono">
                    <p className="text-slate-400 text-sm">TOTAL COMBAT SCORE</p>
                    <p className="text-4xl font-extrabold text-yellow-400">{score}</p>
                    <p className="text-xs text-slate-500">
                      Reached Wave {level}/5 | Max Combo {maxCombo}
                    </p>
                  </div>

                  <form
                    onSubmit={submitScore}
                    className="bg-slate-900/80 p-5 rounded-lg border border-red-500/20 space-y-3"
                  >
                    <label className="block text-xs text-cyan-400 font-mono uppercase tracking-wider font-semibold">
                      Record High Score on Leaderboard
                    </label>
                    <div className="flex gap-2">
                      <input
                        type="text"
                        maxLength={14}
                        placeholder="PILOT ALIAS"
                        value={nickname}
                        onChange={(e) => setNickname(e.target.value)}
                        data-anvil-action="input"
                        className="flex-1 bg-black border border-slate-700 rounded px-3 py-2 text-white font-mono placeholder-slate-600 focus:outline-none focus:border-cyan-400 text-center uppercase tracking-wider text-sm"
                        required
                      />
                      <button
                        type="submit"
                        className="px-5 py-2 bg-pink-600 hover:bg-pink-500 font-mono text-xs font-bold text-white uppercase tracking-wider rounded transition-colors"
                      >
                        SUBMIT
                      </button>
                    </div>
                  </form>

                  <div className="flex gap-3">
                    <button
                      onClick={restartGame}
                      data-anvil-action="restart"
                      className="flex-1 py-3 bg-slate-900 hover:bg-slate-800 border border-slate-700 text-white font-mono text-xs font-bold uppercase tracking-widest rounded transition-colors"
                    >
                      🔄 TRY AGAIN
                    </button>
                    <button
                      onClick={() => setGameState("start")}
                      className="px-5 py-3 bg-slate-950 border border-slate-800 hover:border-slate-700 text-slate-400 hover:text-white font-mono text-xs uppercase rounded transition-colors"
                    >
                      MENU
                    </button>
                  </div>
                </div>
              </div>
            )}
          </div>

          <div className="w-full bg-slate-900/60 border-t border-cyan-500/20 p-4 flex flex-wrap items-center justify-between gap-4 select-none">
            <div className="flex items-center gap-2">
              <button
                onMouseDown={handleMobileLeftStart}
                onMouseUp={handleMobileLeftEnd}
                onTouchStart={handleMobileLeftStart}
                onTouchEnd={handleMobileLeftEnd}
                className="w-16 h-12 bg-slate-800 hover:bg-slate-700 border border-slate-600 rounded-lg flex items-center justify-center active:scale-95 transition-transform"
              >
                <span className="text-xl text-cyan-400">◀</span>
              </button>
              <button
                onMouseDown={handleMobileRightStart}
                onMouseUp={handleMobileRightEnd}
                onTouchStart={handleMobileRightStart}
                onTouchEnd={handleMobileRightEnd}
                className="w-16 h-12 bg-slate-800 hover:bg-slate-700 border border-slate-600 rounded-lg flex items-center justify-center active:scale-95 transition-transform"
              >
                <span className="text-xl text-cyan-400">▶</span>
              </button>
            </div>

            <div className="flex-1 max-w-xs">
              <button
                onClick={handleMobileShoot}
                className="w-full h-12 bg-gradient-to-r from-red-600 to-pink-600 hover:from-red-500 hover:to-pink-500 border border-pink-400 rounded-lg font-mono font-bold tracking-widest text-sm shadow-[0_0_12px_rgba(239,68,68,0.3)] active:scale-95 transition-transform text-white"
              >
                🔥 FIRE WEAPON
              </button>
            </div>

            {gameState === "playing" && (
              <button
                onClick={restartGame}
                data-anvil-action="restart"
                className="px-4 h-12 bg-slate-950 hover:bg-slate-900 border border-slate-800 rounded-lg font-mono text-xs text-slate-400 hover:text-white transition-colors"
              >
                RESTART
              </button>
            )}
          </div>
        </div>

        <aside className="w-full lg:w-80 flex flex-col gap-6">
          <div className="bg-slate-950/80 p-5 rounded-xl border border-pink-500/30 shadow-[0_0_15px_rgba(236,72,153,0.1)] flex flex-col">
            <h3 className="text-md font-mono text-pink-500 font-bold border-b border-pink-500/20 pb-2 mb-3 tracking-widest text-center uppercase">
              🏆 NEON LEADERBOARD
            </h3>

            <div className="space-y-2.5 flex-1">
              {highScores.map((h, i) => (
                <div
                  key={i}
                  className={`flex items-center justify-between font-mono text-xs p-1.5 rounded ${
                    i === 0
                      ? "bg-pink-500/10 border border-pink-500/30 text-pink-300 font-bold"
                      : "text-slate-300"
                  }`}
                >
                  <div className="flex items-center gap-2">
                    <span className="text-slate-500 w-4 text-right">{i + 1}.</span>
                    <span className="truncate max-w-[100px]">{h.name}</span>
                  </div>
                  <div className="flex items-center gap-3">
                    <span className="text-slate-500 text-[10px]">W{h.level}</span>
                    <span className="text-yellow-400 font-bold">{h.score}</span>
                  </div>
                </div>
              ))}
            </div>
          </div>

          <div className="bg-slate-950/80 p-5 rounded-xl border border-slate-800 flex flex-col text-xs font-mono space-y-3">
            <h3 className="text-cyan-400 font-bold border-b border-slate-800 pb-2 mb-1 tracking-wider uppercase">
              ⚡ FIELD DISCOVERIES
            </h3>
            <div className="space-y-2">
              <div className="flex items-start gap-2.5">
                <span className="w-5 h-5 bg-cyan-950 border border-cyan-400 text-cyan-300 rounded flex items-center justify-center font-bold text-[10px]">
                  T
                </span>
                <div>
                  <div className="text-white font-bold text-[11px]">Triple Laser</div>
                  <div className="text-slate-400 text-[10px]">Sweeps wide angles to clear cohorts fast.</div>
                </div>
              </div>

              <div className="flex items-start gap-2.5">
                <span className="w-5 h-5 bg-emerald-950 border border-emerald-400 text-emerald-300 rounded flex items-center justify-center font-bold text-[10px]">
                  L
                </span>
                <div>
                  <div className="text-white font-bold text-[11px]">Mega Beam</div>
                  <div className="text-slate-400 text-[10px]">Unleashes dense green laser at ultra-rapid rate.</div>
                </div>
              </div>

              <div className="flex items-start gap-2.5">
                <span className="w-5 h-5 bg-blue-950 border border-blue-400 text-blue-300 rounded flex items-center justify-center font-bold text-[10px]">
                  S
                </span>
                <div>
                  <div className="text-white font-bold text-[11px]">Aegis Shield</div>
                  <div className="text-slate-400 text-[10px]">Invulnerable shell absorbs enemy bullets.</div>
                </div>
              </div>

              <div className="flex items-start gap-2.5">
                <span className="w-5 h-5 bg-indigo-950 border border-indigo-400 text-indigo-300 rounded flex items-center justify-center font-bold text-[10px]">
                  F
                </span>
                <div>
                  <div className="text-white font-bold text-[11px]">Chrono Freeze</div>
                  <div className="text-slate-400 text-[10px]">Freezes time, slows alien speed by 75%.</div>
                </div>
              </div>

              <div className="flex items-start gap-2.5">
                <span className="w-5 h-5 bg-amber-950 border border-amber-400 text-amber-300 rounded flex items-center justify-center font-bold text-[10px]">
                  B
                </span>
                <div>
                  <div className="text-white font-bold text-[11px]">Nuke Bomb</div>
                  <div className="text-slate-400 text-[10px]">Instantly vaporizes half of all active invaders.</div>
                </div>
              </div>
            </div>
          </div>
        </aside>
      </div>

      <footer className="w-full max-w-4xl text-center border-t border-slate-900 pt-4 mt-8">
        <p className="text-[10px] font-mono text-slate-500 tracking-wider">
          SYSTEM OPERATION PORT: 3011 | BUILT WITH NEXT.JS APP ROUTER & TAILWIND CSS
        </p>
      </footer>
    </main>
  );
}

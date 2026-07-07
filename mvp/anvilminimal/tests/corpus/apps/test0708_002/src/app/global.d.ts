declare module "*.css";

export interface Player {
  x: number;
  y: number;
  width: number;
  height: number;
  speed: number;
}

export interface Alien {
  x: number;
  y: number;
  width: number;
  height: number;
  alive: boolean;
  type: number;
}

export interface Bullet {
  x: number;
  y: number;
  width: number;
  height: number;
  speed: number;
  owner: 'player' | 'alien';
}

export interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  color: string;
}

export interface GameState {
  score: number;
  wave: number;
  lives: number;
  status: 'IDLE' | 'PLAYING' | 'GAME_OVER' | 'WIN';
}

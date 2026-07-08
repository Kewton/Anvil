import { useState, useEffect, useCallback, useRef } from 'react';

export type GamePhase = 'start' | 'playing' | 'game-over';

export interface Entity {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Projectile extends Entity {
  velocity: number;
}

export interface Alien extends Entity {
  type: 'basic' | 'strong' | 'boss';
}

export function useGameEngine() {
  const [phase, setPhase] = useState<GamePhase>('start');
  const [score, setScore] = useState(0);
  const [playerPos, setPlayerPos] = useState({ x: 0, y: 0 });
  const [projectiles, setProjectiles] = useState<Projectile[]>([]);
  const [aliens, setAliens] = useState<Alien[]>([]);
  
  const requestRef = useRef<number>();
  const keysPressed = useRef<Record<string, boolean>>({});
  const alienDirection = useRef(1);
  const alienMoveTimer = useRef(0);

  const CANVAS_WIDTH = 800;
  const CANVAS_HEIGHT = 600;
  const PLAYER_SIZE = 40;
  const ALIEN_SIZE = 30;
  const PROJECTILE_SPEED = 7;
  const ALIEN_SPEED = 1;
  const ALIEN_MOVE_INTERVAL = 30; // frames between moves

  const initAliens = useCallback(() => {
    const newAliens: Alien[] = [];
    const rows = 5;
    const cols = 8;
    const padding = 20;
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        newAliens.push({
          id: `alien-${r}-${c}`,
          x: c * (ALIEN_SIZE + padding) + 100,
          y: r * (ALIEN_SIZE + padding) + 50,
          width: ALIEN_SIZE,
          height: ALIEN_SIZE,
          type: r === 0 ? 'boss' : r < 2 ? 'strong' : 'basic',
        });
      }
    }
    setAliens(newAliens);
  }, []);

  const startGame = useCallback(() => {
    setScore(0);
    setPlayerPos({ x: CANVAS_WIDTH / 2 - PLAYER_SIZE / 2, y: CANVAS_HEIGHT - 60 });
    setProjectiles([]);
    initAliens();
    setPhase('playing');
  }, [initAliens]);

  const restartGame = useCallback(() => {
    startGame();
  }, [startGame]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      keysPressed.current[e.code] = true;
      if (e.code === 'Space' && phase === 'playing') {
        setProjectiles(prev => [...prev, {
          id: Math.random().toString(),
          x: playerPos.x + PLAYER_SIZE / 2 - 2,
          y: playerPos.y,
          width: 4,
          height: 10,
          velocity: -PROJECTILE_SPEED
        }]);
      }
      if (e.code === 'KeyR' && phase === 'game-over') {
        restartGame();
      }
    };
    const handleKeyUp = (e: KeyboardEvent) => {
      keysPressed.current[e.code] = false;
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
    };
  }, [phase, playerPos, restartGame]);

  const update = useCallback(() => {
    if (phase !== 'playing') return;

    // Player movement
    setPlayerPos(prev => {
      let newX = prev.x;
      if (keysPressed.current['ArrowLeft']) newX -= 5;
      if (keysPressed.current['ArrowRight']) newX += 5;
      return {
        x: Math.max(0, Math.min(CANVAS_WIDTH - PLAYER_SIZE, newX)),
        y: prev.y
      };
    });

    // Projectiles movement
    setProjectiles(prev => prev
      .map(p => ({ ...p, y: p.y + p.velocity }))
      .filter(p => p.y + p.height > 0)
    );

    // Aliens movement
    alienMoveTimer.current++;
    if (alienMoveTimer.current >= ALIEN_MOVE_INTERVAL) {
      alienMoveTimer.current = 0;
      
      setAliens(prevAliens => {
        const shouldChangeDir = prevAliens.some(a => 
          (alienDirection.current === 1 && a.x + a.width >= CANVAS_WIDTH) ||
          (alienDirection.current === -1 && a.x <= 0)
        );

        if (shouldChangeDir) {
          alienDirection.current *= -1;
        }

        return prevAliens.map(a => ({
          ...a,
          x: a.x + alienDirection.current * 10,
          y: shouldChangeDir ? a.y + 10 : a.y
        }));
      });
    }

    // Collision Detection
    setProjectiles(prevProjectiles => {
      let hitAny = false;
      const nextProjectiles = prevProjectiles.filter(p => {
        let collision = false;
        setAliens(prevAliens => {
          const nextAliens = prevAliens.filter(a => {
            const isHit = p.x < a.x + a.width &&
                          p.x + p.width > a.x &&
                          p.y < a.y + a.height &&
                          p.y + p.height > a.y;
            if (isHit) {
              collision = true;
              hitAny = true;
              setScore(s => s + (a.type === 'boss' ? 30 : a.type === 'strong' ? 20 : 10));
            }
            return !isHit;
          });
          return nextAliens;
        });
        return !collision;
      });
      return nextProjectiles;
    });

    // Game Over condition: aliens reach player
    setAliens(prevAliens => {
      if (prevAliens.some(a => a.y + a.height >= CANVAS_HEIGHT - 60)) {
        setPhase('game-over');
      }
      return prevAliens;
    });

    // Win condition: all aliens destroyed
    setAliens(prevAliens => {
      if (prevAliens.length === 0 && phase === 'playing') {
        setScore(s => s + 1000);
        setPhase('game-over'); // Simplified: win is game-over for now
      }
      return prevAliens;
    });

    requestRef.current = requestAnimationFrame(update);
  }, [phase, restartGame]);

  useEffect(() => {
    requestRef.current = requestAnimationFrame(update);
    return () => {
      if (requestRef.current) cancelAnimationFrame(requestRef.current);
    };
  }, [update]);

  return {
    phase,
    setPhase,
    score,
    playerPos,
    projectiles,
    aliens,
    startGame,
    restartGame,
    CANVAS_WIDTH,
    CANVAS_HEIGHT
  };
}

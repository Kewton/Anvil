import { useState, useEffect, useCallback, useRef } from 'react';

interface Entity {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Alien extends Entity {
  alive: boolean;
  id: number;
}

interface Bullet extends Entity {
  id: number;
}

interface GameState {
  playerX: number;
  bullets: Bullet[];
  aliens: Alien[];
  alienDirection: number; // 1 for right, -1 for left
  alienStepDown: boolean;
  score: number;
  gameOver: boolean;
  gameStarted: boolean;
}

export function useGameLoop() {
  const [gameState, setGameState] = useState<GameState>({
    playerX: 0,
    bullets: [],
    aliens: [],
    alienDirection: 1,
    alienStepDown: false,
    score: 0,
    gameOver: false,
    gameStarted: false,
  });

  const requestRef = useRef<number>();
  const keysPressed = useRef<{ [key: string]: boolean }>({});
  const canvasWidth = 800;
  const canvasHeight = 600;
  const playerWidth = 40;
  const playerHeight = 20;
  const alienWidth = 30;
  const alienHeight = 20;
  const bulletWidth = 4;
  const bulletHeight = 10;

  const initAliens = useCallback(() => {
    const aliens: Alien[] = [];
    const rows = 5;
    const cols = 11;
    const spacingX = 50;
    const spacingY = 40;
    const offsetX = (canvasWidth - (cols * spacingX)) / 2;
    const offsetY = 50;

    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        aliens.push({
          id: r * cols + c,
          x: offsetX + c * spacingX,
          y: offsetY + r * spacingY,
          width: alienWidth,
          height: alienHeight,
          alive: true,
        });
      }
    }
    return aliens;
  }, []);

  const startGame = useCallback(() => {
    setGameState({
      playerX: (canvasWidth - playerWidth) / 2,
      bullets: [],
      aliens: initAliens(),
      alienDirection: 1,
      alienStepDown: false,
      score: 0,
      gameOver: false,
      gameStarted: true,
    });
  }, [initAliens]);

  const restartGame = useCallback(() => {
    startGame();
  }, [startGame]);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      keysPressed.current[e.code] = true;
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
  }, []);

  const update = useCallback(() => {
    if (!gameState.gameStarted || gameState.gameOver) return;

    setGameState((prev) => {
      let { playerX, bullets, aliens, alienDirection, score, gameOver } = prev;

      // Player movement
      if (keysPressed.current['ArrowLeft']) playerX = Math.max(0, playerX - 5);
      if (keysPressed.current['ArrowRight']) playerX = Math.min(canvasWidth - playerWidth, playerX + 5);

      // Firing (limiting bullet count)
      if (keysPressed.current['Space'] && bullets.length < 3) {
        bullets = [...bullets, {
          id: Date.now(),
          x: playerX + playerWidth / 2 - bulletWidth / 2,
          y: canvasHeight - 40,
          width: bulletWidth,
          height: bulletHeight,
        }];
        // Simple debounce for space
        keysPressed.current['Space'] = false; 
      }

      // Bullet movement
      bullets = bullets.map(b => ({ ...b, y: b.y - 7 })).filter(b => b.y > 0);

      // Alien movement
      let nextDirection = alienDirection;
      let nextStepDown = false;
      let hitWall = false;

      const aliveAliens = aliens.filter(a => a.alive);
      if (aliveAliens.length === 0) {
        // Victory? We can just restart or handle as gameOver: false but win
        return { ...prev, gameOver: true }; 
      }

      const minX = Math.min(...aliveAliens.map(a => a.x));
      const maxX = Math.max(...aliveAliens.map(a => a.x + a.width));

      if (maxX >= canvasWidth - 10 || minX <= 10) {
        hitWall = true;
      }

      if (hitWall) {
        nextDirection = -alienDirection;
        nextStepDown = true;
      }

      const movedAliens = aliens.map(a => {
        if (!a.alive) return a;
        let { x, y } = a;
        if (nextStepDown) {
          y += 20;
        } else {
          x += alienDirection * 2;
        }
        return { ...a, x, y };
      });

      // Collision Detection
      const updatedAliens = movedAliens.map(a => {
        if (!a.alive) return a;
        const hit = bullets.some(b => 
          b.x < a.x + a.width &&
          b.x + b.width > a.x &&
          b.y < a.y + a.height &&
          b.y + b.height > a.y
        );
        return hit ? { ...a, alive: false } : a;
      });

      // Update score
      const killedCount = aliens.filter(a => a.alive).length - updatedAliens.filter(a => a.alive).length;
      score += killedCount * 10;

      // Filter out bullets that hit aliens
      const remainingBullets = bullets.filter(b => {
        return !updatedAliens.some(a => 
          a.alive &&
          b.x < a.x + a.width &&
          b.x + b.width > a.x &&
          b.y < a.y + a.height &&
          b.y + b.height > a.y
        );
      });

      // Check if aliens reached player
      const reachedPlayer = updatedAliens.some(a => a.alive && a.y + a.height >= canvasHeight - 40);
      if (reachedPlayer) {
        gameOver = true;
      }

      return {
        ...prev,
        playerX,
        bullets: remainingBullets,
        aliens: updatedAliens,
        alienDirection: nextDirection,
        alienStepDown: nextStepDown,
        score,
        gameOver,
      };
    });

    requestRef.current = requestAnimationFrame(update);
  }, [gameState.gameStarted, gameState.gameOver]);

  useEffect(() => {
    requestRef.current = requestAnimationFrame(update);
    return () => {
      if (requestRef.current) cancelAnimationFrame(requestRef.current);
    };
  }, [update]);

  return {
    gameState,
    startGame,
    restartGame,
    canvasWidth,
    canvasHeight,
    playerWidth,
    playerHeight,
    alienWidth,
    alienHeight
  };
}

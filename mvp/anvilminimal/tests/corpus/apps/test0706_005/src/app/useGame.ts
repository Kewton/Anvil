import { useEffect, useState, type RefObject } from "react";

type Ship = {
  x: number;
  y: number;
  hp: number;
};

export function useGame(canvasRef: RefObject<HTMLCanvasElement | null>) {
  const [score, setScore] = useState(0);
  const [shield, setShield] = useState(3);
  const [ship, setShip] = useState<Ship>({ x: 220, y: 330, hp: 100 });

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setShip((current) => ({ ...current, x: Math.max(20, current.x - 18) }));
      }
      if (event.key === "ArrowRight") {
        setShip((current) => ({ ...current, x: Math.min(440, current.x + 18) }));
      }
      if (event.key === " ") {
        setScore((current) => current + 5);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    const context = canvas.getContext("2d");
    if (!context) {
      return;
    }

    let animation = 0;
    let frame = 0;
    const enemies = Array.from({ length: 8 }, (_, index) => ({
      x: 44 + index * 52,
      y: 56 + (index % 2) * 38,
    }));

    const draw = () => {
      frame += 1;
      context.fillStyle = "#050816";
      context.fillRect(0, 0, canvas.width, canvas.height);
      context.fillStyle = "#38bdf8";
      context.fillRect(ship.x, ship.y, 42, 18);
      context.fillStyle = "#f97316";
      for (const enemy of enemies) {
        context.fillRect(enemy.x + Math.sin(frame / 20) * 12, enemy.y, 28, 18);
      }
      context.fillStyle = "#f8fafc";
      context.font = "18px monospace";
      context.fillText(`Score ${score} Shield ${shield}`, 18, 28);
      animation = requestAnimationFrame(draw);
    };

    draw();
    return () => cancelAnimationFrame(animation);
  }, [canvasRef, score, shield, ship]);

  const restart = () => {
    setScore(0);
    setShield(3);
    setShip({ x: 220, y: 330, hp: 100 });
  };

  return { score, shield, ship, restart };
}

"use client";
import { useState, useEffect, useCallback } from "react";
import { Invader, Bullet } from "@/types/game";

export default function Home() {
  const [shipPos, setShipPos] = useState(50);
  const [invaders, setInvaders] = useState<Invader[]>([]);
  const [bullets, setBullets] = useState<Bullet[]>([]);
  const [score, setScore] = useState(0);
  const [highScore, setHighScore] = useState(0);
  const [gameState, setGameState] = useState<'START' | 'PLAYING' | 'GAMEOVER'>('START');
  const [shake, setShake] = useState(false);

  useEffect(() => {
    const saved = localStorage.getItem('invadersHighScore');
    if (saved) setHighScore(parseInt(saved));
  }, []);

  const initGame = useCallback(() => {
    const initialInvaders = [];
    for (let row = 0; row < 3; row++) {
      for (let col = 0; col < 8; col++) {
        initialInvaders.push({ id: row * 8 + col, x: 15 + col * 10, y: 10 + row * 10 });
      }
    }
    setInvaders(initialInvaders);
    setBullets([]);
    setScore(0);
    setShipPos(50);
    setGameState('PLAYING');
  }, []);

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (gameState !== 'PLAYING') return;
      if (e.key === "ArrowLeft") setShipPos((p) => Math.max(5, p - 3));
      if (e.key === "ArrowRight") setShipPos((p) => Math.min(95, p + 3));
      if (e.key === " ") {
        setBullets((b) => [...b, { id: Date.now(), x: shipPos, y: 85 }]);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [shipPos, gameState]);

  useEffect(() => {
    if (gameState !== 'PLAYING') return;
    const loop = setInterval(() => {
      setBullets((b) => b.map((bullet) => ({ ...bullet, y: bullet.y - 2 })).filter((b) => b.y > 0));
      
      setBullets((b) => {
        const nextBullets = [...b];
        let hit = false;
        setInvaders((invs) => {
          const nextInvaders = invs.filter((inv) => {
            const hitIndex = nextBullets.findIndex(
              (bullet) => Math.abs(bullet.x - inv.x) < 4 && Math.abs(bullet.y - inv.y) < 4
            );
            if (hitIndex !== -1) {
              setScore((s) => {
                const newScore = s + 100;
                if (newScore > highScore) {
                    setHighScore(newScore);
                    localStorage.setItem('invadersHighScore', newScore.toString());
                }
                return newScore;
              });
              nextBullets.splice(hitIndex, 1);
              hit = true;
              return false;
            }
            return true;
          });
          return nextInvaders;
        });
        if (hit) {
          setShake(true);
          setTimeout(() => setShake(false), 200);
        }
        return nextBullets;
      });

      setInvaders((invs) => invs.map(inv => ({ ...inv, y: inv.y + 0.1 })));
      
      if (invaders.some(inv => inv.y > 85)) setGameState('GAMEOVER');
      if (invaders.length === 0) setGameState('GAMEOVER');
    }, 50);
    return () => clearInterval(loop);
  }, [gameState, invaders, highScore]);

  return (
    <div className={`relative h-screen w-full bg-black overflow-hidden ${shake ? 'translate-x-2' : ''}`}>
      {gameState !== 'PLAYING' && (
        <div className="absolute inset-0 flex flex-col items-center justify-center bg-black/90 z-50 text-white p-8 border-4 border-green-500 shadow-[0_0_50px_#22c55e]">
          <h1 className="text-6xl font-bold text-transparent bg-clip-text bg-gradient-to-r from-green-400 to-blue-500 mb-8 tracking-widest">NEON INVADERS</h1>
          {gameState === 'GAMEOVER' && <h2 className="text-4xl text-red-500 mb-4">GAME OVER</h2>}
          <div className="text-xl mb-8">HIGH SCORE: {highScore}</div>
          <button 
            onClick={initGame} 
            className="px-8 py-4 bg-green-600 text-white font-bold text-2xl hover:bg-green-700 transition-all shadow-[0_0_20px_#15803d]"
          >
            {gameState === 'START' ? 'START MISSION' : 'RESTART'}
          </button>
        </div>
      )}
      
      <div className="absolute top-4 left-4 text-green-400 font-mono text-xl">SCORE: {score}</div>
      
      {/* Ship */}
      <div className="absolute bottom-10 w-12 h-12 bg-cyan-400 shadow-[0_0_20px_#22d3ee] transition-all duration-75" style={{ left: `${shipPos}%`, transform: 'translateX(-50%)' }} />
      
      {/* Invaders */}
      {invaders.map((inv) => (
        <div key={inv.id} className="absolute w-8 h-8 bg-purple-600 rounded-sm shadow-[0_0_15px_#9333ea]" style={{ left: `${inv.x}%`, top: `${inv.y}%` }} />
      ))}
      
      {/* Bullets */}
      {bullets.map((b) => (
        <div key={b.id} className="absolute w-2 h-6 bg-yellow-300 shadow-[0_0_10px_#fde047]" style={{ left: `${b.x}%`, top: `${b.y}%` }} />
      ))}
    </div>
  );
}

"use client";

import { useState } from "react";

export default function Page() {
  const [playerX, setPlayerX] = useState(0);
  const [score, setScore] = useState(3);
  const [gameState, setGameState] = useState("PLAYING");
  const [alienFleet, setAlienFleet] = useState([{ x: 20, y: 30 }]);
  let playerScore = 0;
  playerScore += 10;

  const CheckCollision = () => true;
  if (CheckCollision()) {
    setGameState("GAMEOVER");
  }

  const RestartGame = () => {
    setScore(0);
    setAlienFleet([{ x: 20, y: 30 }]);
    setGameState("PLAYING");
  };

  return (
    <main tabIndex={0} onKeyDown={() => setPlayerX((value) => value + 1)}>
      <button onClick={RestartGame}>Restart</button>
      <canvas />
      <p>
        score {score + playerScore} alien {alienFleet.length} collision {gameState} x {playerX}
      </p>
    </main>
  );
}

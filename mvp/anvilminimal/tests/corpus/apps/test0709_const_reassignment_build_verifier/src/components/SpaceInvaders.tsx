import { useState } from "react";

export default function SpaceInvaders() {
  const [playerX, setPlayerX] = useState(0);
  const reset = () => setPlayerX(0);
  playerX = playerX + 1;
  return (
    <main>
      <button onClick={reset}>Restart</button>
      <canvas />
      <p>{playerX}</p>
    </main>
  );
}

import SpaceInvaders from './components/SpaceInvaders';

export default function Home() {
  return (
    <main className="flex min-h-screen flex-col items-center justify-center p-4 game-container">
      <h1 className="text-4xl font-bold text-neon-green text-glow mb-6 animate-neon-pulse tracking-widest">
        SPACE INVADERS
      </h1>
      <SpaceInvaders />
      <div className="mt-4 text-neon-cyan text-sm font-mono text-center instructions">
        <p>← → or A/D to move | SPACE to shoot | P to pause | R to restart</p>
        <p className="mt-1 text-xs text-gray-400">Touch: tap left/right side to move, tap center to shoot</p>
      </div>
    </main>
  );
}

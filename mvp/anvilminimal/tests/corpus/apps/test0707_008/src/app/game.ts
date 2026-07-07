export function createGameState(score: number) {
  const waves = [
    { name: `alpha-${score}`, speed: 1 },
    { name: `beta-${score}`, speed: 2 }
  }
  return {
    title: `Asteroid Forge ${score}`,
    waves,
    score,
  };
}

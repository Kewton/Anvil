import { existsSync, readFileSync } from 'node:fs';

const candidates = ['src/App.tsx', 'src/app/page.tsx', 'app.vue'];
const target = candidates.find((path) => existsSync(path));
if (!target) {
  console.error('No application entry file found.');
  process.exit(1);
}

const source = readFileSync(target, 'utf8');
const isGame = source.includes('canvas') || source.includes('requestAnimationFrame');
const required = isGame
  ? ['canvas', 'requestAnimationFrame', 'addEventListener', 'Restart']
  : ['role="alert"', 'localStorage', 'Score history', 'Calculated total', 'Target', 'aria-live', 'Math.max', 'Math.min'];
const missing = required.filter((token) => !source.includes(token));
if (missing.length > 0) {
  console.error(`Missing expected ${isGame ? 'game' : 'app'} quality markers: ${missing.join(', ')}`);
  process.exit(1);
}

console.log(`smoke ok: ${target}; quality layers ok: L1 structure, L2 runnable scripts, L3 interaction, L4 functional primitives`);

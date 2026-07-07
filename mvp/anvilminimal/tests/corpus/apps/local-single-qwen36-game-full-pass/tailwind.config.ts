import type { Config } from 'tailwindcss';

const config: Config = {
  content: [
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
    extend: {
      colors: {
        'neon-green': '#39ff14',
        'neon-pink': '#ff6ec7',
        'neon-cyan': '#00ffff',
        'neon-yellow': '#ffff00',
        'neon-orange': '#ff6600',
        'space-dark': '#0a0a1a',
        'space-mid': '#1a1a3a',
        'space-light': '#2a2a5a',
      },
      fontFamily: {
        'pixel': ['"Press Start 2P"', 'monospace'],
      },
      animation: {
        'blink': 'blink 1s step-end infinite',
        'float': 'float 3s ease-in-out infinite',
        'pulse-glow': 'pulseGlow 2s ease-in-out infinite',
        'scan': 'scan 4s linear infinite',
      },
      keyframes: {
        blink: {
          '0%, 100%': { opacity: '1' },
          '50%': { opacity: '0' },
        },
        float: {
          '0%, 100%': { transform: 'translateY(0)' },
          '50%': { transform: 'translateY(-10px)' },
        },
        pulseGlow: {
          '0%, 100%': { boxShadow: '0 0 5px #39ff14, 0 0 10px #39ff14' },
          '50%': { boxShadow: '0 0 20px #39ff14, 0 0 30px #39ff14' },
        },
        scan: {
          '0%': { transform: 'translateY(-100%)' },
          '100%': { transform: 'translateY(100vh)' },
        },
      },
    },
  },
  plugins: [],
};

export default config;

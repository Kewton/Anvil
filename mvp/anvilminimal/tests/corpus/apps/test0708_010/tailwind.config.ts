import type { Config } from 'tailwindcss';

const config: Config = {
  content: [
    './src/pages/**/*.{js,ts,jsx,tsx,mdx}',
    './src/components/**/*.{js,ts,jsx,tsx,mdx}',
    './src/app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
    extend: {
      colors: {
        neon: {
          green: '#39ff14',
          pink: '#ff6ec7',
          blue: '#00d4ff',
          yellow: '#ffd700',
          red: '#ff073a',
        },
      },
      animation: {
        'pulse-fast': 'pulse 0.5s ease-in-out infinite',
        'glow': 'glow 2s ease-in-out infinite alternate',
      },
      keyframes: {
        glow: {
          '0%': { textShadow: '0 0 5px #39ff14, 0 0 10px #39ff14' },
          '100%': { textShadow: '0 0 10px #39ff14, 0 0 20px #39ff14, 0 0 30px #39ff14' },
        },
      },
    },
  },
  plugins: [],
};

export default config;

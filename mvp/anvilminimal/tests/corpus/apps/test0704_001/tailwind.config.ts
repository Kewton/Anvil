import type { Config } from "tailwindcss";

const config: Config = {
  content: [
    "./src/pages/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/components/**/*.{js,ts,jsx,tsx,mdx}",
    "./src/app/**/*.{js,ts,jsx,tsx,mdx}",
  ],
  theme: {
    extend: {
      colors: {
        neon: {
          green: "#39FF14",
          pink: "#FF00FF",
          blue: "#00FFFF",
          purple: "#BC13FE",
        },
      },
      animation: {
        "pulse-slow": "pulse 3s cubic-bezier(0.4, 0, 0.6, 1) infinite",
        "glow": "glow 1.5s ease-in-out infinite alternate",
      },
      keyframes: {
        glow: {
          "0%": { textShadow: "0 0 5px #fff, 0 0 10px #fff, 0 0 20px #00ffff" },
          "100%": { textShadow: "0 0 10px #fff, 0 0 20px #ff00ff, 0 0 40px #ff00ff" },
        },
      },
      boxShadow: {
        "neon": "0 0 10px #39FF14, 0 0 20px #39FF14",
      },
    },
  },
  plugins: [],
};
export default config;

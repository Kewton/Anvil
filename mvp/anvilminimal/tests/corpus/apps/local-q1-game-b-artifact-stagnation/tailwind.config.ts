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
        space: {
          bg: "#0a0e27",
          primary: "#00f5ff",
          secondary: "#bf00ff",
          accent: "#ff006e",
          star: "#ffffff",
        },
      },
    },
  },
  plugins: [],
};

export default config;

import type { Config } from "tailwindcss";
import plugin from "tailwindcss/plugin";

export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        brand: {
          50: "var(--brand-50)",
          100: "var(--brand-100)",
          200: "var(--brand-200)",
          300: "var(--brand-300)",
          400: "var(--brand-400)",
          500: "var(--brand-500)",
          600: "var(--brand-600)",
          700: "var(--brand-700)",
          800: "var(--brand-800)",
          900: "var(--brand-900)",
          950: "var(--brand-950)",
        },
      },
    },
  },
  plugins: [
    plugin(function ({ addUtilities }) {
      addUtilities({
        ".bg-surface": {
          "background-color": "var(--surface)",
        },
        ".bg-surface-alt": {
          "background-color": "var(--surface-alt)",
        },
        ".bg-tertiary": {
          "background-color": "var(--tertiary)",
        },
        ".bg-hover": {
          "background-color": "var(--hover)",
        },
        ".text-primary": {
          color: "var(--text-primary)",
        },
        ".text-secondary": {
          color: "var(--text-secondary)",
        },
        ".border-themed": {
          "border-color": "var(--border)",
        },
        ".border-themed-faint": {
          "border-color": "var(--border-faint)",
        },
      });
    }),
  ],
} satisfies Config;

import { useState, useEffect, useCallback } from "react";

export type ThemeMode = "system" | "light" | "dark";
export type ThemeName = "default" | "ocean" | "ember" | "forest" | "violet" | "terminal" | "graphite" | "brutus" | "meridian" | "editorial";

interface ThemeState {
  mode: ThemeMode;
  theme: ThemeName;
  resolved: "light" | "dark";
}

const STORAGE_KEY = "coalesce-theme";

function getSystemPreference(): "light" | "dark" {
  if (typeof window === "undefined") return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function loadSaved(): { mode: ThemeMode; theme: ThemeName } {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return {
        mode: parsed.mode ?? "system",
        theme: parsed.theme ?? "default",
      };
    }
  } catch {}
  return { mode: "system", theme: "default" };
}

// Google Fonts per motif theme (loaded on demand)
const THEME_FONTS: Partial<Record<ThemeName, string>> = {
  terminal: "Share+Tech+Mono&family=JetBrains+Mono:wght@300;400;500;600",
  graphite: "Inter:wght@300;400;500;600;700",
  brutus: "Barlow+Condensed:wght@700;900&family=Barlow:wght@400;500&family=Roboto+Mono:wght@400;500",
  meridian: "Orbitron:wght@400;700;900&family=Exo+2:wght@300;400;500&family=Share+Tech+Mono",
  editorial: "Cormorant+Garamond:ital,wght@0,300;0,600;1,300&family=DM+Sans:wght@300;400;500&family=DM+Mono:wght@300;400",
};

const loadedFonts = new Set<string>();

function loadThemeFonts(theme: ThemeName) {
  const families = THEME_FONTS[theme];
  if (!families || loadedFonts.has(theme)) return;
  loadedFonts.add(theme);
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = `https://fonts.googleapis.com/css2?family=${families}&display=swap`;
  document.head.appendChild(link);
}

// Terminal is dark-only
const DARK_ONLY_THEMES: ThemeName[] = ["terminal"];

function applyTheme(resolved: "light" | "dark", theme: ThemeName) {
  const root = document.documentElement;
  root.classList.remove("light", "dark");
  const actual = DARK_ONLY_THEMES.includes(theme) ? "dark" : resolved;
  root.classList.add(actual);
  root.setAttribute("data-theme", theme);
  loadThemeFonts(theme);
}

export function useTheme() {
  const [state, setState] = useState<ThemeState>(() => {
    const saved = loadSaved();
    const resolved = saved.mode === "system" ? getSystemPreference() : saved.mode;
    return { ...saved, resolved };
  });

  // Listen for system preference changes
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => {
      if (state.mode === "system") {
        const resolved = getSystemPreference();
        setState((s) => ({ ...s, resolved }));
      }
    };
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, [state.mode]);

  // Apply on change
  useEffect(() => {
    applyTheme(state.resolved, state.theme);
  }, [state.resolved, state.theme]);

  // Persist on change
  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ mode: state.mode, theme: state.theme }));
  }, [state.mode, state.theme]);

  const setMode = useCallback((mode: ThemeMode) => {
    const resolved = mode === "system" ? getSystemPreference() : mode;
    setState((s) => ({ ...s, mode, resolved }));
  }, []);

  const setTheme = useCallback((theme: ThemeName) => {
    setState((s) => ({ ...s, theme }));
  }, []);

  return { ...state, setMode, setTheme };
}

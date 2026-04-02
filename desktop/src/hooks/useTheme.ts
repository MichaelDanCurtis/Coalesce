import { useState, useEffect, useCallback } from "react";

export type ThemeMode = "system" | "light" | "dark";
export type ThemeName = "default" | "ocean" | "ember" | "forest" | "violet";

interface ThemeState {
  mode: ThemeMode;
  theme: ThemeName;
  resolved: "light" | "dark";
}

const STORAGE_KEY = "agentpather-theme";

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

function applyTheme(resolved: "light" | "dark", theme: ThemeName) {
  const root = document.documentElement;
  root.classList.remove("light", "dark");
  root.classList.add(resolved);
  root.setAttribute("data-theme", theme);
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

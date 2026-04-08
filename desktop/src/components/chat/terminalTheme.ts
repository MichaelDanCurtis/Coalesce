// Phase 19.7: terminal theme settings, persisted to localStorage.
import type { CSSProperties } from "react";

export type TerminalFont = "JetBrains Mono" | "Fira Code" | "SF Mono" | "Menlo";
export type TerminalScheme = "green" | "amber" | "white" | "coalesce";

export interface TerminalTheme {
  font: TerminalFont;
  scanlines: boolean;
  scheme: TerminalScheme;
}

const KEY = "coalesce_terminal_theme";

export const DEFAULT_TERMINAL_THEME: TerminalTheme = {
  font: "JetBrains Mono",
  scanlines: true,
  scheme: "green",
};

export const TERMINAL_FONTS: TerminalFont[] = [
  "JetBrains Mono",
  "Fira Code",
  "SF Mono",
  "Menlo",
];

export const TERMINAL_SCHEMES: Array<{ id: TerminalScheme; label: string }> = [
  { id: "green", label: "Phosphor Green" },
  { id: "amber", label: "Amber CRT" },
  { id: "white", label: "Paper White" },
  { id: "coalesce", label: "Coalesce Brand" },
];

export function loadTerminalTheme(): TerminalTheme {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) return { ...DEFAULT_TERMINAL_THEME, ...JSON.parse(raw) };
  } catch {
    /* ignore */
  }
  return { ...DEFAULT_TERMINAL_THEME };
}

export function saveTerminalTheme(theme: TerminalTheme) {
  localStorage.setItem(KEY, JSON.stringify(theme));
  window.dispatchEvent(new CustomEvent("coalesce:terminal-theme", { detail: theme }));
}

const SCHEME_VARS: Record<TerminalScheme, Record<string, string>> = {
  green: {
    "--term-bg": "#0b0f0a",
    "--term-bg-alt": "#0f140d",
    "--term-fg": "#c5e8b0",
    "--term-fg-dim": "#6b8a5a",
    "--term-accent": "#7fff7f",
    "--term-accent-dim": "#4fbf4f",
  },
  amber: {
    "--term-bg": "#120b02",
    "--term-bg-alt": "#1a1004",
    "--term-fg": "#ffcf7a",
    "--term-fg-dim": "#8a6531",
    "--term-accent": "#ffb86c",
    "--term-accent-dim": "#c78a3f",
  },
  white: {
    "--term-bg": "#f5f5f0",
    "--term-bg-alt": "#eaeae4",
    "--term-fg": "#1a1a1a",
    "--term-fg-dim": "#666666",
    "--term-accent": "#0066cc",
    "--term-accent-dim": "#004c99",
  },
  coalesce: {
    "--term-bg": "#0a0f1a",
    "--term-bg-alt": "#0f1523",
    "--term-fg": "#cfe0ff",
    "--term-fg-dim": "#6b7ea0",
    "--term-accent": "#7fa7ff",
    "--term-accent-dim": "#4f7fcf",
  },
};

/// Inline style object for the chat root. Combines scheme colors and
/// font family so the existing .chat-terminal CSS picks them up.
export function terminalThemeStyle(theme: TerminalTheme): CSSProperties {
  const vars = SCHEME_VARS[theme.scheme] ?? SCHEME_VARS.green;
  return {
    ...(vars as CSSProperties),
    fontFamily: `"${theme.font}", "JetBrains Mono", "Fira Code", "SF Mono", "Menlo", monospace`,
  };
}

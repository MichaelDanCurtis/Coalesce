import { useState, useEffect, useCallback } from "react";
import type { ThemeMode, ThemeName } from "../hooks/useTheme";
import { useTranslation, type Locale } from "../i18n";

interface ThemeHook {
  mode: ThemeMode;
  theme: ThemeName;
  resolved: "light" | "dark";
  setMode: (m: ThemeMode) => void;
  setTheme: (t: ThemeName) => void;
}

const THEMES: { id: ThemeName; label: string; accent: string; preview: string }[] = [
  { id: "default", label: "Sky", accent: "bg-sky-500", preview: "Default blue theme" },
  { id: "ocean", label: "Ocean", accent: "bg-cyan-500", preview: "Deep teal tones" },
  { id: "ember", label: "Ember", accent: "bg-orange-500", preview: "Warm orange tones" },
  { id: "forest", label: "Forest", accent: "bg-emerald-500", preview: "Natural green tones" },
  { id: "violet", label: "Violet", accent: "bg-violet-500", preview: "Purple accent theme" },
];

const MODES: { id: ThemeMode; label: string; desc: string }[] = [
  { id: "system", label: "System", desc: "Follow OS preference" },
  { id: "light", label: "Light", desc: "Always light" },
  { id: "dark", label: "Dark", desc: "Always dark" },
];

const LANGUAGES: { id: Locale; label: string }[] = [
  { id: "en", label: "English" },
  { id: "zh", label: "\u4E2D\u6587" },
  { id: "ja", label: "\u65E5\u672C\u8A9E" },
];

function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI__" in window;
}

export default function Settings({ theme }: { theme: ThemeHook }) {
  const { t, locale, setLocale } = useTranslation();

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">{t("nav.settings")}</h2>

      {/* Appearance */}
      <div className="card">
        <h3 className="text-sm font-medium mb-4">{t("settings.appearance")}</h3>

        {/* Mode */}
        <div className="mb-4">
          <p className="text-xs text-secondary mb-2">{t("settings.mode")}</p>
          <div className="flex gap-2">
            {MODES.map((m) => (
              <button
                key={m.id}
                onClick={() => theme.setMode(m.id)}
                className={`flex-1 rounded-lg border p-3 text-left transition-colors ${
                  theme.mode === m.id
                    ? "border-brand-500 bg-brand-500/10"
                    : "border-themed hover:border-brand-500/50"
                }`}
              >
                <p className="text-sm font-medium">{m.label}</p>
                <p className="text-xs text-secondary">{m.desc}</p>
              </button>
            ))}
          </div>
        </div>

        {/* Theme */}
        <div className="mb-4">
          <p className="text-xs text-secondary mb-2">{t("settings.theme")}</p>
          <div className="flex gap-2">
            {THEMES.map((th) => (
              <button
                key={th.id}
                onClick={() => theme.setTheme(th.id)}
                className={`flex-1 rounded-lg border p-3 text-center transition-colors ${
                  theme.theme === th.id
                    ? "border-brand-500 bg-brand-500/10"
                    : "border-themed hover:border-brand-500/50"
                }`}
              >
                <div className={`h-4 w-4 rounded-full ${th.accent} mx-auto mb-1`} />
                <p className="text-xs font-medium">{th.label}</p>
              </button>
            ))}
          </div>
        </div>

        {/* Language */}
        <div>
          <p className="text-xs text-secondary mb-2">{t("settings.language")}</p>
          <div className="flex gap-2">
            {LANGUAGES.map((lang) => (
              <button
                key={lang.id}
                onClick={() => setLocale(lang.id)}
                className={`flex-1 rounded-lg border p-3 text-center transition-colors ${
                  locale === lang.id
                    ? "border-brand-500 bg-brand-500/10"
                    : "border-themed hover:border-brand-500/50"
                }`}
              >
                <p className="text-sm font-medium">{lang.label}</p>
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Proxy Config */}
      <div className="card">
        <h3 className="text-sm font-medium mb-4">{t("settings.proxy")}</h3>
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-sm">Proxy Address</p>
              <p className="text-xs text-secondary">Where the proxy server is running</p>
            </div>
            <code className="rounded bg-tertiary px-3 py-1 text-sm">localhost:8402</code>
          </div>
          <div className="flex items-center justify-between">
            <div>
              <p className="text-sm">Config File</p>
              <p className="text-xs text-secondary">Edit the TOML config to add providers</p>
            </div>
            <code className="rounded bg-tertiary px-3 py-1 text-sm">coalesce.toml</code>
          </div>
        </div>
      </div>

      {/* Startup & Updates */}
      <div className="card">
        <h3 className="text-sm font-medium mb-4">{t("settings.startup")}</h3>
        <div className="space-y-4">
          <AutoStartToggle />
          <UpdateChecker />
        </div>
      </div>

      <div className="card">
        <h3 className="text-sm font-medium mb-4">{t("settings.about")}</h3>
        <div className="space-y-2 text-sm text-secondary">
          <p>Coalesce v0.1.0</p>
          <p>Smart LLM routing proxy with dynamic cost optimization</p>
          <p className="text-xs opacity-60">Built with Rust + Tauri 2 + React</p>
        </div>
      </div>
    </div>
  );
}

function AutoStartToggle() {
  const [enabled, setEnabled] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    (async () => {
      try {
        const { isEnabled } = await import("@tauri-apps/plugin-autostart");
        setEnabled(await isEnabled());
      } catch {}
    })();
  }, []);

  const toggle = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true);
    try {
      const { enable, disable, isEnabled } = await import("@tauri-apps/plugin-autostart");
      if (enabled) {
        await disable();
      } else {
        await enable();
      }
      setEnabled(await isEnabled());
    } catch {}
    setLoading(false);
  }, [enabled]);

  if (!isTauri()) return null;

  return (
    <div className="flex items-center justify-between">
      <div>
        <p className="text-sm">Launch at Login</p>
        <p className="text-xs text-secondary">Start Coalesce when you log in</p>
      </div>
      <button
        onClick={toggle}
        disabled={loading}
        className={`relative inline-flex h-6 w-11 items-center rounded-full transition-colors ${
          enabled ? "bg-brand-500" : "bg-tertiary"
        }`}
      >
        <span
          className={`inline-block h-4 w-4 rounded-full bg-white transition-transform ${
            enabled ? "translate-x-6" : "translate-x-1"
          }`}
        />
      </button>
    </div>
  );
}

function UpdateChecker() {
  const [status, setStatus] = useState<string>("");
  const [checking, setChecking] = useState(false);

  const checkForUpdates = useCallback(async () => {
    if (!isTauri()) return;
    setChecking(true);
    setStatus("Checking...");
    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (update) {
        setStatus(`Update available: v${update.version}`);
      } else {
        setStatus("You're on the latest version");
      }
    } catch {
      setStatus("Update check unavailable");
    }
    setChecking(false);
  }, []);

  if (!isTauri()) return null;

  return (
    <div className="flex items-center justify-between">
      <div>
        <p className="text-sm">Check for Updates</p>
        {status && <p className="text-xs text-secondary">{status}</p>}
      </div>
      <button
        onClick={checkForUpdates}
        disabled={checking}
        className="rounded-lg border border-themed px-3 py-1.5 text-sm hover:border-brand-500/50 transition-colors disabled:opacity-50"
      >
        {checking ? "Checking..." : "Check Now"}
      </button>
    </div>
  );
}

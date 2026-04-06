import { useState } from "react";
import type { ThemeMode, ThemeName } from "../hooks/useTheme";
import { useTranslation } from "../i18n";

const tabs = [
  { id: "chat", labelKey: "nav.chat" },
  { id: "overview", labelKey: "nav.overview" },
  { id: "providers", labelKey: "nav.providers" },
  { id: "config", labelKey: "nav.config" },
  { id: "localllm", labelKey: "nav.localllm" },
  { id: "costs", labelKey: "nav.costs" },
  { id: "usage", labelKey: "nav.usage" },
  { id: "playground", labelKey: "nav.playground" },
  { id: "timeline", labelKey: "nav.timeline" },
  { id: "prompts", labelKey: "nav.prompts" },
  { id: "events", labelKey: "nav.events" },
  { id: "mcp", labelKey: "nav.mcp" },
  { id: "skills", labelKey: "nav.skills" },
  { id: "plugins", labelKey: "nav.plugins" },
  { id: "sync", labelKey: "nav.sync" },
  { id: "settings", labelKey: "nav.settings" },
] as const;

type TabId = (typeof tabs)[number]["id"];

interface LayoutProps {
  activeTab: TabId;
  onTabChange: (tab: TabId) => void;
  themeMode: ThemeMode;
  themeName: ThemeName;
  onThemeModeChange: (mode: ThemeMode) => void;
  children: React.ReactNode;
}

export type { TabId };

const modeIcons: Record<ThemeMode, string> = {
  system: "S",
  light: "L",
  dark: "D",
};

export default function Layout({ activeTab, onTabChange, themeMode, onThemeModeChange, children }: LayoutProps) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(false);

  const cycleMode = () => {
    const modes: ThemeMode[] = ["system", "light", "dark"];
    const idx = modes.indexOf(themeMode);
    onThemeModeChange(modes[(idx + 1) % modes.length]);
  };

  return (
    <div className="flex h-screen bg-surface text-primary">
      {/* Sidebar */}
      <aside className={`flex flex-col border-r border-themed bg-surface-alt transition-all duration-200 ${collapsed ? "w-12" : "w-56"}`}>
        <div className={`flex items-center ${collapsed ? "justify-center px-1 py-3" : "gap-2 px-4 py-5"}`}>
          <button
            onClick={() => setCollapsed(!collapsed)}
            className="h-8 w-8 rounded-lg bg-brand-600 flex items-center justify-center text-sm font-bold text-white shrink-0 cursor-pointer hover:bg-brand-500 transition-colors"
            title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          >
            {collapsed ? (
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 6h16M4 12h16M4 18h16" />
              </svg>
            ) : (
              "AP"
            )}
          </button>
          {!collapsed && (
            <div>
              <h1 className="text-sm font-semibold">Coalesce</h1>
              <p className="text-xs text-secondary">LLM Router</p>
            </div>
          )}
        </div>

        <nav className={`flex-1 space-y-0.5 ${collapsed ? "px-1" : "px-2"} overflow-y-auto`}>
          {tabs.map((tab) => (
            <button
              key={tab.id}
              onClick={() => onTabChange(tab.id)}
              className={`w-full rounded-lg ${collapsed ? "px-0 py-2 text-center" : "px-3 py-2 text-left"} text-sm transition-colors ${
                activeTab === tab.id
                  ? "bg-brand-600/10 text-brand-400 font-medium"
                  : "text-secondary hover:bg-hover hover:text-primary"
              }`}
              title={collapsed ? t(tab.labelKey) : undefined}
            >
              {collapsed ? t(tab.labelKey).charAt(0).toUpperCase() : t(tab.labelKey)}
            </button>
          ))}
        </nav>

        {!collapsed && (
          <div className="border-t border-themed p-4 space-y-3">
            {/* Theme Toggle */}
            <button
              onClick={cycleMode}
              className="flex items-center gap-2 w-full text-xs text-secondary hover:text-primary transition-colors"
              title={`Theme: ${themeMode}`}
            >
              <span className="h-5 w-5 rounded bg-tertiary flex items-center justify-center text-[10px] font-bold">
                {modeIcons[themeMode]}
              </span>
              <span>{themeMode === "system" ? "System" : themeMode === "light" ? "Light" : "Dark"} mode</span>
            </button>

            <div className="flex items-center gap-2">
              <div className="h-2 w-2 rounded-full bg-emerald-500" />
              <span className="text-xs text-secondary">Proxy: localhost:8402</span>
            </div>
          </div>
        )}

        {collapsed && (
          <div className="border-t border-themed py-3 flex flex-col items-center gap-2">
            <button
              onClick={cycleMode}
              className="h-5 w-5 rounded bg-tertiary flex items-center justify-center text-[10px] font-bold text-secondary hover:text-primary cursor-pointer"
              title={`Theme: ${themeMode}`}
            >
              {modeIcons[themeMode]}
            </button>
            <div className="h-2 w-2 rounded-full bg-emerald-500" title="Proxy: localhost:8402" />
          </div>
        )}
      </aside>

      {/* Main content */}
      <main className={`flex-1 overflow-auto ${activeTab === "chat" ? "" : "p-6"}`}>{children}</main>
    </div>
  );
}

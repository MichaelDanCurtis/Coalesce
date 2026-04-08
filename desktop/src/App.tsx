import { useState, useEffect } from "react";
import Layout, { type TabId } from "./components/Layout";
import Overview from "./components/Overview";
import Providers from "./components/Providers";
import ProviderConfig from "./components/ProviderConfig";
import LocalLLM from "./components/LocalLLM";
import CostAnalytics from "./components/CostAnalytics";
import UsageMetrics from "./components/UsageMetrics";
import RoutingPlayground from "./components/RoutingPlayground";
import RequestTimeline from "./components/RequestTimeline";
import SystemPrompts from "./components/SystemPrompts";
import EventStream from "./components/EventStream";
import McpPanel from "./components/McpPanel";
import SkillsPanel from "./components/SkillsPanel";
import PluginsPanel from "./components/PluginsPanel";
import CloudSync from "./components/CloudSync";
import Settings from "./components/Settings";
import Chat from "./components/Chat";
import { useTheme } from "./hooks/useTheme";
import { I18nProvider } from "./i18n";
import { NotificationProvider } from "./components/Notifications";

function App() {
  const [activeTab, setActiveTab] = useState<TabId>("overview");
  const theme = useTheme();

  // Allow any component to request a tab switch (Phase 19.6
  // Send-to-Chat). Listen on window to avoid plumbing props.
  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<{ tab: TabId }>).detail;
      if (detail?.tab) setActiveTab(detail.tab);
    };
    window.addEventListener("coalesce:set-tab", handler);
    return () => window.removeEventListener("coalesce:set-tab", handler);
  }, []);

  const renderTab = () => {
    switch (activeTab) {
      case "chat":
        return <Chat />;
      case "overview":
        return <Overview />;
      case "providers":
        return <Providers />;
      case "config":
        return <ProviderConfig />;
      case "localllm":
        return <LocalLLM />;
      case "costs":
        return <CostAnalytics />;
      case "usage":
        return <UsageMetrics />;
      case "playground":
        return <RoutingPlayground />;
      case "timeline":
        return <RequestTimeline />;
      case "prompts":
        return <SystemPrompts />;
      case "events":
        return <EventStream />;
      case "mcp":
        return <McpPanel />;
      case "skills":
        return <SkillsPanel />;
      case "plugins":
        return <PluginsPanel />;
      case "sync":
        return <CloudSync />;
      case "settings":
        return <Settings theme={theme} />;
    }
  };

  return (
    <I18nProvider>
      <NotificationProvider>
        <Layout
          activeTab={activeTab}
          onTabChange={setActiveTab}
          themeMode={theme.mode}
          themeName={theme.theme}
          onThemeModeChange={theme.setMode}
        >
          {renderTab()}
        </Layout>
      </NotificationProvider>
    </I18nProvider>
  );
}

export default App;

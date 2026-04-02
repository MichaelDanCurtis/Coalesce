import { useState } from "react";
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
import Settings from "./components/Settings";
import Chat from "./components/Chat";
import { useTheme } from "./hooks/useTheme";
import { I18nProvider } from "./i18n";
import { NotificationProvider } from "./components/Notifications";

function App() {
  const [activeTab, setActiveTab] = useState<TabId>("overview");
  const theme = useTheme();

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

import { createContext, useContext, useState, type ReactNode } from "react";

type Locale = "en" | "zh" | "ja";

const translations: Record<Locale, Record<string, string>> = {
  en: {
    "nav.overview": "Overview",
    "nav.providers": "Providers",
    "nav.config": "Provider Config",
    "nav.costs": "Cost Analytics",
    "nav.usage": "Usage Metrics",
    "nav.playground": "Routing Playground",
    "nav.timeline": "Request Timeline",
    "nav.prompts": "System Prompts",
    "nav.events": "Live Events",
    "nav.localllm": "Local LLM",
    "nav.settings": "Settings",
    "localllm.title": "Local LLM Management",
    "localllm.running": "Running Models",
    "localllm.installed": "Installed Models",
    "localllm.pull": "Pull New Model",
    "localllm.pull_btn": "Pull",
    "localllm.pulling": "Pulling...",
    "localllm.pull_hint": "Enter a model name from the Ollama registry (ollama.com/library)",
    "localllm.refresh": "Refresh",
    "localllm.no_running": "No models currently loaded in memory",
    "localllm.no_models": "No models installed. Pull one above to get started.",
    "localllm.col_model": "Model",
    "localllm.col_family": "Family",
    "localllm.col_size": "Size",
    "localllm.col_params": "Params",
    "localllm.col_routed": "Routed",
    "localllm.col_actions": "Actions",
    "localllm.status_running": "Running",
    "localllm.status_stopped": "Stopped",
    "localllm.start": "Start Ollama",
    "localllm.stop": "Stop",
    "localllm.gpu_info": "GPU Hardware",
    "localllm.resources": "Resource Monitor",
    "localllm.system_memory": "System Memory",
    "localllm.browse": "Model Browser",
    "localllm.search_placeholder": "Search models...",
    "localllm.import_gguf": "Import GGUF Model",
    "localllm.import_hint": "Import a local .gguf file as an Ollama model",
    "localllm.gguf_path": "GGUF File Path",
    "localllm.model_name": "Model Name",
    "localllm.import_btn": "Import",
    "overview.title": "Overview",
    "overview.total_requests": "Total Requests",
    "overview.success_rate": "Success Rate",
    "overview.money_saved": "Money Saved",
    "overview.avg_latency": "Avg Latency",
    "overview.providers": "Providers",
    "overview.recent": "Recent Requests",
    "settings.appearance": "Appearance",
    "settings.mode": "Mode",
    "settings.theme": "Color Theme",
    "settings.language": "Language",
    "settings.proxy": "Proxy Configuration",
    "settings.startup": "Startup & Updates",
    "settings.about": "About",
    "common.loading": "Loading...",
    "common.offline": "Proxy Offline",
  },
  zh: {
    "nav.overview": "概览",
    "nav.providers": "提供商",
    "nav.config": "提供商配置",
    "nav.costs": "成本分析",
    "nav.usage": "使用量",
    "nav.playground": "路由测试",
    "nav.timeline": "请求时间线",
    "nav.prompts": "系统提示词",
    "nav.events": "实时事件",
    "nav.localllm": "本地模型",
    "nav.settings": "设置",
    "localllm.title": "本地LLM管理",
    "localllm.running": "运行中的模型",
    "localllm.installed": "已安装的模型",
    "localllm.pull": "拉取新模型",
    "localllm.pull_btn": "拉取",
    "localllm.pulling": "拉取中...",
    "localllm.pull_hint": "输入Ollama注册表中的模型名称 (ollama.com/library)",
    "localllm.refresh": "刷新",
    "localllm.no_running": "当前没有模型加载在内存中",
    "localllm.no_models": "没有已安装的模型，在上方拉取一个开始使用",
    "localllm.col_model": "模型",
    "localllm.col_family": "系列",
    "localllm.col_size": "大小",
    "localllm.col_params": "参数",
    "localllm.col_routed": "路由",
    "localllm.col_actions": "操作",
    "localllm.status_running": "运行中",
    "localllm.status_stopped": "已停止",
    "localllm.start": "启动 Ollama",
    "localllm.stop": "停止",
    "localllm.gpu_info": "GPU 硬件",
    "localllm.resources": "资源监控",
    "localllm.system_memory": "系统内存",
    "localllm.browse": "模型浏览",
    "localllm.search_placeholder": "搜索模型...",
    "localllm.import_gguf": "导入 GGUF 模型",
    "localllm.import_hint": "将本地 .gguf 文件导入为 Ollama 模型",
    "localllm.gguf_path": "GGUF 文件路径",
    "localllm.model_name": "模型名称",
    "localllm.import_btn": "导入",
    "overview.title": "概览",
    "overview.total_requests": "总请求数",
    "overview.success_rate": "成功率",
    "overview.money_saved": "节省金额",
    "overview.avg_latency": "平均延迟",
    "overview.providers": "提供商",
    "overview.recent": "最近请求",
    "settings.appearance": "外观",
    "settings.mode": "模式",
    "settings.theme": "颜色主题",
    "settings.language": "语言",
    "settings.proxy": "代理配置",
    "settings.startup": "启动与更新",
    "settings.about": "关于",
    "common.loading": "加载中...",
    "common.offline": "代理离线",
  },
  ja: {
    "nav.overview": "概要",
    "nav.providers": "プロバイダ",
    "nav.config": "プロバイダ設定",
    "nav.costs": "コスト分析",
    "nav.usage": "使用量",
    "nav.playground": "ルーティングテスト",
    "nav.timeline": "リクエスト履歴",
    "nav.prompts": "システムプロンプト",
    "nav.events": "ライブイベント",
    "nav.localllm": "ローカルLLM",
    "nav.settings": "設定",
    "localllm.title": "ローカルLLM管理",
    "localllm.running": "実行中のモデル",
    "localllm.installed": "インストール済みモデル",
    "localllm.pull": "新しいモデルを取得",
    "localllm.pull_btn": "取得",
    "localllm.pulling": "取得中...",
    "localllm.pull_hint": "Ollamaレジストリのモデル名を入力 (ollama.com/library)",
    "localllm.refresh": "更新",
    "localllm.no_running": "現在メモリにロードされているモデルはありません",
    "localllm.no_models": "モデルがインストールされていません。上から取得してください。",
    "localllm.col_model": "モデル",
    "localllm.col_family": "ファミリー",
    "localllm.col_size": "サイズ",
    "localllm.col_params": "パラメータ",
    "localllm.col_routed": "ルーティング",
    "localllm.col_actions": "操作",
    "localllm.status_running": "実行中",
    "localllm.status_stopped": "停止中",
    "localllm.start": "Ollama を起動",
    "localllm.stop": "停止",
    "localllm.gpu_info": "GPU ハードウェア",
    "localllm.resources": "リソースモニター",
    "localllm.system_memory": "システムメモリ",
    "localllm.browse": "モデルブラウザ",
    "localllm.search_placeholder": "モデルを検索...",
    "localllm.import_gguf": "GGUF モデルのインポート",
    "localllm.import_hint": "ローカルの .gguf ファイルを Ollama モデルとしてインポート",
    "localllm.gguf_path": "GGUF ファイルパス",
    "localllm.model_name": "モデル名",
    "localllm.import_btn": "インポート",
    "overview.title": "概要",
    "overview.total_requests": "総リクエスト",
    "overview.success_rate": "成功率",
    "overview.money_saved": "節約額",
    "overview.avg_latency": "平均遅延",
    "overview.providers": "プロバイダ",
    "overview.recent": "最近のリクエスト",
    "settings.appearance": "外観",
    "settings.mode": "モード",
    "settings.theme": "カラーテーマ",
    "settings.language": "言語",
    "settings.proxy": "プロキシ設定",
    "settings.startup": "起動とアップデート",
    "settings.about": "について",
    "common.loading": "読み込み中...",
    "common.offline": "プロキシオフライン",
  },
};

interface I18nContextType {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: string) => string;
}

const I18nContext = createContext<I18nContextType>({
  locale: "en",
  setLocale: () => {},
  t: (k) => k,
});

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(() => {
    const saved = localStorage.getItem("coalesce-locale");
    return (saved as Locale) || "en";
  });

  const t = (key: string): string => {
    return translations[locale]?.[key] ?? translations.en[key] ?? key;
  };

  const updateLocale = (l: Locale) => {
    setLocale(l);
    localStorage.setItem("coalesce-locale", l);
  };

  return (
    <I18nContext.Provider value={{ locale, setLocale: updateLocale, t }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useTranslation() {
  return useContext(I18nContext);
}

export type { Locale };

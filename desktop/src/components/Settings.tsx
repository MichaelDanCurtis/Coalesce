import { useState, useEffect, useCallback } from "react";
import type { ThemeMode, ThemeName } from "../hooks/useTheme";
import { useTranslation, type Locale } from "../i18n";
import { api } from "../api/client";

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

      {/* Harness Configuration */}
      <HarnessConfig />

      {/* Auto-Failover Rules */}
      <FailoverRules />

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

interface HarnessInfo {
  id: string;
  name: string;
  icon: string;
  installed: boolean;
  configured: boolean;
  config_path: string | null;
  backup_exists: boolean;
  description: string;
}

function HarnessConfig() {
  const [harnesses, setHarnesses] = useState<HarnessInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [message, setMessage] = useState<{ text: string; ok: boolean } | null>(null);

  const fetchHarnesses = useCallback(async () => {
    try {
      const data = await api.getHarnesses() as any;
      setHarnesses(data.harnesses || []);
    } catch {
      setHarnesses([]);
    }
    setLoading(false);
  }, []);

  useEffect(() => { fetchHarnesses(); }, [fetchHarnesses]);

  const handleConfigure = async (id: string) => {
    setActionLoading(id);
    setMessage(null);
    try {
      const result = await api.configureHarness(id) as any;
      setMessage({ text: result.message, ok: result.success });
      await fetchHarnesses();
    } catch (e: any) {
      setMessage({ text: e.message || "Failed to configure", ok: false });
    }
    setActionLoading(null);
  };

  const handleRestore = async (id: string) => {
    setActionLoading(id);
    setMessage(null);
    try {
      const result = await api.restoreHarness(id) as any;
      setMessage({ text: result.message, ok: result.success });
      await fetchHarnesses();
    } catch (e: any) {
      setMessage({ text: e.message || "Failed to restore", ok: false });
    }
    setActionLoading(null);
  };

  return (
    <div className="card">
      <h3 className="text-sm font-medium mb-2">AI Tool Integration</h3>
      <p className="text-xs text-secondary mb-4">
        Connect your AI coding tools to route through Coalesce. Each tool's config is backed up before changes.
      </p>

      {message && (
        <div className={`rounded-lg px-3 py-2 text-xs mb-3 ${
          message.ok ? "bg-emerald-500/10 text-emerald-400" : "bg-red-500/10 text-red-400"
        }`}>
          {message.text}
        </div>
      )}

      {loading ? (
        <p className="text-xs text-secondary">Detecting installed tools...</p>
      ) : (
        <div className="space-y-3">
          {harnesses.map((h) => (
            <div key={h.id} className="flex items-center justify-between rounded-lg border border-themed p-3">
              <div className="flex items-center gap-3">
                <div className={`flex h-9 w-9 items-center justify-center rounded-lg text-xs font-bold ${
                  h.configured
                    ? "bg-emerald-500/20 text-emerald-400"
                    : h.installed
                    ? "bg-brand-500/20 text-brand-400"
                    : "bg-tertiary text-secondary"
                }`}>
                  {h.icon}
                </div>
                <div>
                  <div className="flex items-center gap-2">
                    <p className="text-sm font-medium">{h.name}</p>
                    {h.configured && (
                      <span className="rounded-full bg-emerald-500/20 px-2 py-0.5 text-[10px] font-medium text-emerald-400">
                        Connected
                      </span>
                    )}
                    {!h.installed && (
                      <span className="rounded-full bg-tertiary px-2 py-0.5 text-[10px] font-medium text-secondary">
                        Not Installed
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-secondary">{h.description}</p>
                  {h.config_path && h.installed && (
                    <p className="text-[10px] text-secondary opacity-60 mt-0.5 font-mono">{h.config_path}</p>
                  )}
                </div>
              </div>
              <div className="flex gap-2">
                {h.installed && !h.configured && (
                  <button
                    onClick={() => handleConfigure(h.id)}
                    disabled={actionLoading === h.id}
                    className="rounded-lg bg-brand-500 px-3 py-1.5 text-xs font-medium text-white hover:bg-brand-600 transition-colors disabled:opacity-50"
                  >
                    {actionLoading === h.id ? "Connecting..." : "Connect"}
                  </button>
                )}
                {h.configured && (
                  <button
                    onClick={() => handleRestore(h.id)}
                    disabled={actionLoading === h.id}
                    className="rounded-lg border border-themed px-3 py-1.5 text-xs hover:border-red-500/50 hover:text-red-400 transition-colors disabled:opacity-50"
                  >
                    {actionLoading === h.id ? "Restoring..." : "Disconnect"}
                  </button>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function FailoverRules() {
  const [rules, setRules] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);

  const fetchRules = useCallback(async () => {
    try {
      const data = await api.getRules();
      setRules((data as any).rules || []);
    } catch { setRules([]); }
    setLoading(false);
  }, []);

  useEffect(() => { fetchRules(); }, [fetchRules]);

  const loadPresets = async () => {
    try {
      const data = await api.getRulePresets();
      for (const preset of (data as any).presets || []) {
        await api.createRule(preset);
      }
      await fetchRules();
    } catch {}
  };

  const handleToggle = async (id: string) => {
    await api.toggleRule(id);
    await fetchRules();
  };

  const handleDelete = async (id: string) => {
    await api.deleteRule(id);
    await fetchRules();
  };

  const handleCreateRule = async (rule: any) => {
    await api.createRule(rule);
    await fetchRules();
    setShowForm(false);
  };

  const conditionLabel = (c: any) => {
    switch (c.type) {
      case "quota_below": return `${c.provider} quota < ${c.percent}%`;
      case "error_rate_above": return `${c.provider} errors > ${c.threshold}%`;
      case "latency_above": return `${c.provider} latency > ${c.ms}ms`;
      case "budget_above": return `Budget > ${c.percent}%`;
      case "provider_down": return `${c.provider} is down`;
      default: return JSON.stringify(c);
    }
  };

  const actionLabel = (a: any) => {
    switch (a.type) {
      case "disable_provider": return `Disable ${a.provider}`;
      case "prefer_provider": return `Prefer ${a.provider} (pri ${a.priority})`;
      case "notify": return `Notify: ${a.message}`;
      case "switch_profile": return `Switch to "${a.profile}" profile`;
      default: return JSON.stringify(a);
    }
  };

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-sm font-medium">Auto-Failover Rules</h3>
        <div className="flex items-center gap-2">
          {rules.length === 0 && !loading && (
            <button onClick={loadPresets} className="rounded-lg border border-themed px-3 py-1 text-xs hover:border-brand-500/50 transition-colors">
              Load Presets
            </button>
          )}
          <button onClick={() => setShowForm(!showForm)} className="rounded-lg border border-themed px-3 py-1 text-xs hover:border-brand-500/50 transition-colors">
            {showForm ? "Cancel" : "+ New Rule"}
          </button>
        </div>
      </div>
      <p className="text-xs text-secondary mb-4">
        Automatic actions triggered by provider conditions. Rules are evaluated on each request.
      </p>

      {showForm && <CreateRuleForm onSubmit={handleCreateRule} onCancel={() => setShowForm(false)} />}

      {loading ? (
        <p className="text-xs text-secondary">Loading rules...</p>
      ) : rules.length === 0 ? (
        <p className="text-xs text-secondary">No rules configured. Load presets or create a new rule.</p>
      ) : (
        <div className="space-y-2">
          {rules.map((rule) => (
            <div key={rule.id} className={`flex items-center justify-between rounded-lg border p-3 ${
              rule.enabled ? "border-brand-500/30 bg-brand-500/5" : "border-themed opacity-60"
            }`}>
              <div className="flex-1 min-w-0">
                <p className="text-sm font-medium truncate">{rule.name}</p>
                <p className="text-xs text-secondary mt-0.5">
                  If {conditionLabel(rule.condition)} → {actionLabel(rule.action)}
                </p>
              </div>
              <div className="flex items-center gap-2 ml-3">
                <button
                  onClick={() => handleToggle(rule.id)}
                  className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                    rule.enabled ? "bg-brand-500" : "bg-tertiary"
                  }`}
                >
                  <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
                    rule.enabled ? "translate-x-4.5" : "translate-x-0.5"
                  }`} />
                </button>
                <button
                  onClick={() => handleDelete(rule.id)}
                  className="text-xs text-secondary hover:text-red-400 transition-colors"
                >
                  Del
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function CreateRuleForm({ onSubmit, onCancel }: { onSubmit: (rule: any) => void; onCancel: () => void }) {
  const [name, setName] = useState("");
  const [condType, setCondType] = useState("error_rate_above");
  const [condProvider, setCondProvider] = useState("");
  const [condThreshold, setCondThreshold] = useState("50");
  const [actionType, setActionType] = useState("disable_provider");
  const [actionProvider, setActionProvider] = useState("");
  const [actionValue, setActionValue] = useState("");

  const needsCondProvider = ["quota_below", "error_rate_above", "latency_above", "provider_down"].includes(condType);
  const needsActionProvider = ["disable_provider", "prefer_provider"].includes(actionType);

  const buildCondition = () => {
    switch (condType) {
      case "quota_below": return { type: condType, provider: condProvider, percent: parseFloat(condThreshold) };
      case "error_rate_above": return { type: condType, provider: condProvider, threshold: parseFloat(condThreshold) };
      case "latency_above": return { type: condType, provider: condProvider, ms: parseInt(condThreshold) };
      case "budget_above": return { type: condType, percent: parseFloat(condThreshold) };
      case "provider_down": return { type: condType, provider: condProvider };
      default: return { type: condType };
    }
  };

  const buildAction = () => {
    switch (actionType) {
      case "disable_provider": return { type: actionType, provider: actionProvider };
      case "prefer_provider": return { type: actionType, provider: actionProvider, priority: parseInt(actionValue || "5") };
      case "notify": return { type: actionType, message: actionValue || "Rule triggered" };
      case "switch_profile": return { type: actionType, profile: actionValue || "default" };
      default: return { type: actionType };
    }
  };

  const handleSubmit = () => {
    if (!name.trim()) return;
    onSubmit({
      id: `rule-${Date.now()}`,
      name: name.trim(),
      enabled: true,
      condition: buildCondition(),
      action: buildAction(),
    });
  };

  const selectClass = "w-full rounded-lg border border-themed bg-primary px-2 py-1.5 text-xs focus:border-brand-500/50 focus:outline-none";
  const inputClass = "w-full rounded-lg border border-themed bg-primary px-2 py-1.5 text-xs focus:border-brand-500/50 focus:outline-none";

  const condThresholdLabel = condType === "latency_above" ? "Threshold (ms)" : condType === "provider_down" ? "" : "Threshold (%)";

  return (
    <div className="rounded-lg border border-brand-500/30 bg-brand-500/5 p-3 mb-3 space-y-3">
      <input
        type="text" placeholder="Rule name" value={name} onChange={(e) => setName(e.target.value)}
        className={inputClass}
      />

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="text-xs text-secondary mb-1 block">When...</label>
          <select value={condType} onChange={(e) => setCondType(e.target.value)} className={selectClass}>
            <option value="error_rate_above">Error rate above</option>
            <option value="latency_above">Latency above</option>
            <option value="quota_below">Quota below</option>
            <option value="budget_above">Budget above</option>
            <option value="provider_down">Provider down</option>
          </select>
        </div>
        {needsCondProvider && (
          <div>
            <label className="text-xs text-secondary mb-1 block">Provider</label>
            <input type="text" placeholder="e.g. openai" value={condProvider} onChange={(e) => setCondProvider(e.target.value)} className={inputClass} />
          </div>
        )}
      </div>

      {condThresholdLabel && (
        <input type="number" placeholder={condThresholdLabel} value={condThreshold} onChange={(e) => setCondThreshold(e.target.value)} className={inputClass} />
      )}

      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="text-xs text-secondary mb-1 block">Then...</label>
          <select value={actionType} onChange={(e) => setActionType(e.target.value)} className={selectClass}>
            <option value="disable_provider">Disable provider</option>
            <option value="prefer_provider">Prefer provider</option>
            <option value="notify">Send notification</option>
            <option value="switch_profile">Switch profile</option>
          </select>
        </div>
        {needsActionProvider && (
          <div>
            <label className="text-xs text-secondary mb-1 block">Target provider</label>
            <input type="text" placeholder="e.g. deepseek" value={actionProvider} onChange={(e) => setActionProvider(e.target.value)} className={inputClass} />
          </div>
        )}
      </div>

      {(actionType === "prefer_provider" || actionType === "notify" || actionType === "switch_profile") && (
        <input
          type="text"
          placeholder={actionType === "prefer_provider" ? "Priority (e.g. 5)" : actionType === "notify" ? "Notification message" : "Profile name"}
          value={actionValue} onChange={(e) => setActionValue(e.target.value)}
          className={inputClass}
        />
      )}

      <div className="flex gap-2">
        <button onClick={handleSubmit} className="rounded-lg bg-brand-500 px-3 py-1.5 text-xs text-white hover:bg-brand-600 transition-colors">
          Create Rule
        </button>
        <button onClick={onCancel} className="rounded-lg border border-themed px-3 py-1.5 text-xs hover:border-brand-500/50 transition-colors">
          Cancel
        </button>
      </div>
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

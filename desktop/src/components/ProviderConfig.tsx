import { useState, useCallback, useEffect, useRef } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";

const DISPLAY_NAMES: Record<string, string> = {
  google: "Google", ollama: "Ollama", openrouter: "OpenRouter", openai: "OpenAI",
  anthropic: "Anthropic", deepseek: "DeepSeek", kimi: "Kimi", glm: "GLM",
  xai: "xAI", copilot: "Copilot", "copilot-personal": "Copilot Personal",
  "copilot-work": "Copilot Work",
};
export function displayName(id: string): string {
  return DISPLAY_NAMES[id] ?? id.charAt(0).toUpperCase() + id.slice(1);
}

const KNOWN_PROVIDERS: ProviderDef[] = [
  { id: "ollama", label: "Ollama (Local)", kind: "local", billing: "local", desc: "Run models locally via Ollama" },
  { id: "copilot", label: "GitHub Copilot", kind: "oauth", billing: "quota_metered:50:18000", desc: "Free quota then per-token (add multiple accounts)" },
  { id: "openrouter", label: "OpenRouter", kind: "apikey", billing: "per_token", envVar: "OPENROUTER_API_KEY", desc: "Access 200+ models with one key" },
  { id: "anthropic", label: "Anthropic", kind: "apikey", billing: "per_token", envVar: "ANTHROPIC_API_KEY", desc: "Claude models direct" },
  { id: "openai", label: "OpenAI", kind: "apikey", billing: "per_token", envVar: "OPENAI_API_KEY", desc: "GPT-4o, o1, o3 direct" },
  { id: "google", label: "Google Gemini", kind: "google_auth", billing: "quota_only:50:0", envVar: "GOOGLE_API_KEY", desc: "Gemini models — quota-only, stops when exhausted" },
  { id: "deepseek", label: "DeepSeek", kind: "apikey", billing: "per_token", envVar: "DEEPSEEK_API_KEY", desc: "DeepSeek V3 and R1" },
  { id: "kimi", label: "Kimi / Moonshot", kind: "apikey", billing: "unlimited", envVar: "KIMI_API_KEY", desc: "Moonshot AI models" },
  { id: "xai", label: "xAI / Grok", kind: "apikey", billing: "per_token", envVar: "XAI_API_KEY", desc: "Grok models" },
  { id: "glm", label: "GLM / Zhipu", kind: "apikey", billing: "per_token", envVar: "GLM_API_KEY", desc: "GLM-4 and ChatGLM" },
];

interface ProviderDef {
  id: string; label: string; kind: "local" | "oauth" | "apikey" | "google_auth";
  billing: string; envVar?: string; desc: string;
}

export default function ProviderConfig() {
  const fetchProviders = useCallback(() => api.getProviders(), []);
  const fetchHealth = useCallback(() => api.getHealth(), []);
  const { data: providersData, loading, error, refresh } = useApi(fetchProviders);
  const { data: healthData } = useApi(fetchHealth);

  if (loading) return <div className="flex items-center justify-center h-64 text-secondary">Loading...</div>;
  if (error) return <OfflineMessage />;

  const activeProviders = (providersData as any)?.providers ?? [];
  const health = healthData as any;
  const breakers = health?.circuit_breakers ?? [];
  const activeNames = new Set(activeProviders.map((p: any) => p.name));

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Provider Configuration</h2>

      {/* Active Providers */}
      {activeProviders.length > 0 && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Active Providers</h3>
          <div className="space-y-3">
            {activeProviders.map((p: any) => (
              <ActiveProviderCard
                key={p.name}
                provider={p}
                breaker={breakers.find((b: any) => b.provider === p.name)}
                onRemoved={refresh}
              />
            ))}
          </div>
        </div>
      )}

      {/* Copilot Account Priority (if multiple copilot accounts) */}
      {(() => {
        const copilotAccounts = activeProviders.filter((p: any) => p.name === "copilot" || p.name.startsWith("copilot-"));
        if (copilotAccounts.length > 1) {
          return (
            <div className="card">
              <h3 className="text-sm font-medium mb-2">Copilot Account Priority</h3>
              <p className="text-xs text-secondary mb-3">
                When multiple Copilot accounts are available, the router tries them in this order.
                Drag to reorder or use the arrows.
              </p>
              <div className="space-y-2">
                {copilotAccounts.map((p: any, i: number) => (
                  <div key={p.name} className="flex items-center gap-2 bg-surface-alt rounded px-3 py-2">
                    <span className="text-xs font-mono text-brand-400 w-5">{i + 1}.</span>
                    <span className="text-sm flex-1">{p.name}</span>
                    <span className="text-xs text-secondary">{p.models ?? 0} models</span>
                    <button
                      onClick={() => {
                        if (i === 0) return;
                        const reordered = [...copilotAccounts];
                        [reordered[i - 1], reordered[i]] = [reordered[i], reordered[i - 1]];
                        // Update pin order for all tiers that reference copilot
                        api.getRoutingPins().then(data => {
                          const pins = data.pins ?? {};
                          for (const tier of Object.keys(pins)) {
                            for (const pin of pins[tier]) {
                              const copilotProvs = pin.providers.filter((pr: string) => pr === "copilot" || pr.startsWith("copilot-"));
                              if (copilotProvs.length > 1) {
                                const nonCopilot = pin.providers.filter((pr: string) => pr !== "copilot" && !pr.startsWith("copilot-"));
                                const orderedCopilot = reordered.map((a: any) => a.name).filter((n: string) => copilotProvs.includes(n));
                                pin.providers = [...orderedCopilot, ...nonCopilot];
                              }
                            }
                          }
                          api.setRoutingPins(pins);
                        });
                        refresh();
                      }}
                      disabled={i === 0}
                      className="text-xs text-secondary hover:text-primary disabled:opacity-20"
                    >{"\u25B2"}</button>
                    <button
                      onClick={() => {
                        if (i === copilotAccounts.length - 1) return;
                        const reordered = [...copilotAccounts];
                        [reordered[i], reordered[i + 1]] = [reordered[i + 1], reordered[i]];
                        api.getRoutingPins().then(data => {
                          const pins = data.pins ?? {};
                          for (const tier of Object.keys(pins)) {
                            for (const pin of pins[tier]) {
                              const copilotProvs = pin.providers.filter((pr: string) => pr === "copilot" || pr.startsWith("copilot-"));
                              if (copilotProvs.length > 1) {
                                const nonCopilot = pin.providers.filter((pr: string) => pr !== "copilot" && !pr.startsWith("copilot-"));
                                const orderedCopilot = reordered.map((a: any) => a.name).filter((n: string) => copilotProvs.includes(n));
                                pin.providers = [...orderedCopilot, ...nonCopilot];
                              }
                            }
                          }
                          api.setRoutingPins(pins);
                        });
                        refresh();
                      }}
                      disabled={i === copilotAccounts.length - 1}
                      className="text-xs text-secondary hover:text-primary disabled:opacity-20"
                    >{"\u25BC"}</button>
                  </div>
                ))}
              </div>
            </div>
          );
        }
        return null;
      })()}

      {/* Ollama Model Manager (if ollama is active) */}
      {activeNames.has("ollama") && <OllamaModelManager />}

      {/* Add Providers */}
      <div>
        <h3 className="text-sm font-medium text-secondary mb-3">Add a Provider</h3>
        <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
          {KNOWN_PROVIDERS.filter(kp => {
            // Copilot supports multiple accounts — always show it
            if (kp.id === "copilot") return true;
            return !activeNames.has(kp.id);
          }).map(kp => (
            <SetupCard key={kp.id} def={kp} onAdded={refresh} />
          ))}
        </div>
      </div>
    </div>
  );
}

/* ─── Active Provider Card ─── */

function ActiveProviderCard({ provider: p, breaker: cb, onRemoved }: {
  provider: any; breaker: any; onRemoved: () => void;
}) {
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; msg: string } | null>(null);
  const [removing, setRemoving] = useState(false);
  const [priority, setPriority] = useState<number>(p.priority ?? 50);
  const [billing, setBilling] = useState<string>(p.billing ?? "per_token");

  const testConn = async () => {
    setTesting(true);
    try {
      const r = await api.testProvider(p.name) as any;
      setTestResult({ ok: r.ok, msg: r.ok ? r.message : r.error });
    } catch { setTestResult({ ok: false, msg: "Connection failed" }); }
    setTesting(false);
  };

  const [confirmRemove, setConfirmRemove] = useState(false);
  const remove = async () => {
    if (!confirmRemove) { setConfirmRemove(true); return; }
    setRemoving(true);
    try { await api.deleteProvider(p.name); onRemoved(); }
    catch {} finally { setRemoving(false); setConfirmRemove(false); }
  };

  const updateSettings = async (newPriority: number, newBilling: string) => {
    setPriority(newPriority);
    setBilling(newBilling);
    try {
      await api.setProviderPriorities({ [p.name]: { priority: newPriority, pricing_mode: newBilling } });
      // Update the economics engine's billing type
      await fetch(`http://127.0.0.1:8402/api/v1/providers/${p.name}/billing`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ billing: newBilling }),
      });
    } catch {}
  };

  const rawCb = cb?.circuit_breaker?.state ?? cb?.state;
  const cbState = rawCb === "Closed" ? "Healthy" : rawCb === "Open" ? "Unavailable" : rawCb === "HalfOpen" ? "Recovering" : rawCb;

  // Map billing string to dropdown value
  const billingCategory = (b: string) => {
    if (!b || b === "unknown") return "per_token";
    if (b === "local") return "local";
    if (b === "unlimited") return "unlimited";
    if (b.startsWith("quota_only")) return "quota_only";
    if (b.startsWith("quota_metered") || b.startsWith("quota_refreshing")) return "quota_metered";
    if (b.startsWith("free_credits")) return "quota_only";
    return "per_token";
  };

  const currentCategory = billingCategory(billing);
  const isLocal = p.name === "ollama";

  return (
    <div className="card">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className={`h-3 w-3 rounded-full ${p.is_available ? "bg-emerald-500" : "bg-red-500"}`} />
          <div>
            <p className="font-medium">{displayName(p.name)}</p>
            <p className="text-xs text-secondary">
              {p.model_count} models
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {cbState && <span className={cbState === "Healthy" ? "badge-green" : cbState === "Recovering" ? "badge-yellow" : "badge-red"}>{cbState}</span>}
          <button onClick={testConn} disabled={testing} className="btn-ghost text-xs">
            {testing ? "Testing..." : "Test"}
          </button>
          <button onClick={remove} disabled={removing} className="btn-ghost text-xs text-red-400 hover:text-red-300"
            onBlur={() => setConfirmRemove(false)}>
            {removing ? "Removing..." : confirmRemove ? "Confirm?" : "Remove"}
          </button>
        </div>
      </div>

      {/* Priority & Billing */}
      <div className="mt-2 flex flex-wrap items-center gap-4 text-xs">
        <label className="flex items-center gap-1.5 text-secondary">
          Priority:
          <select
            value={priority}
            onChange={e => updateSettings(Number(e.target.value), billing)}
            className="rounded border border-themed bg-tertiary px-1.5 py-0.5 text-xs"
          >
            <option value={1}>1 — Use first</option>
            <option value={10}>10 — High</option>
            <option value={25}>25 — Above average</option>
            <option value={50}>50 — Default</option>
            <option value={75}>75 — Below average</option>
            <option value={90}>90 — Last resort</option>
          </select>
        </label>
        {!isLocal && (
          <label className="flex items-center gap-1.5 text-secondary">
            Billing:
            <select
              value={currentCategory}
              onChange={e => {
                const val = e.target.value;
                let billingStr = val;
                if (val === "quota_only") billingStr = "quota_only:50:0";
                else if (val === "quota_metered") billingStr = "quota_metered:50:18000";
                updateSettings(priority, billingStr);
              }}
              className="rounded border border-themed bg-tertiary px-1.5 py-0.5 text-xs"
            >
              <option value="quota_only">Quota-only (stops when exhausted)</option>
              <option value="quota_metered">Quota + metered (free then per-token)</option>
              <option value="per_token">Metered (per-token only)</option>
            </select>
          </label>
        )}
      </div>

      {testResult && (
        <div className={`mt-2 text-xs ${testResult.ok ? "text-emerald-400" : "text-red-400"}`}>
          {testResult.msg}
        </div>
      )}
      {cb && (
        <div className="mt-2 grid grid-cols-3 gap-4 text-xs text-secondary">
          <span>Requests: {cb.total_requests}</span>
          <span>Failures: {cb.total_failures}</span>
          <span>Consecutive fails: {cb.failures}</span>
        </div>
      )}
    </div>
  );
}

/* ─── Setup Card (inactive providers) ─── */

function SetupCard({ def, onAdded }: { def: ProviderDef; onAdded: () => void }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="card">
      <div className="flex items-center justify-between cursor-pointer" onClick={() => setExpanded(!expanded)}>
        <div>
          <p className="text-sm font-medium">{def.label}</p>
          <p className="text-xs text-secondary">{def.desc}</p>
        </div>
        <span className="text-xs text-secondary">{expanded ? "▲" : "▼"}</span>
      </div>
      {expanded && (
        <div className="mt-3 pt-3 border-t border-themed">
          {def.kind === "oauth" && <CopilotWizard onComplete={onAdded} />}
          {def.kind === "apikey" && <ApiKeySetup def={def} onComplete={onAdded} />}
          {def.kind === "local" && <OllamaSetup onComplete={onAdded} />}
          {def.kind === "google_auth" && <GoogleSetup def={def} onComplete={onAdded} />}
        </div>
      )}
    </div>
  );
}

/* ─── Copilot OAuth Wizard ─── */

function CopilotWizard({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<"idle" | "waiting" | "complete" | "error">("idle");
  const [userCode, setUserCode] = useState("");
  const [verifyUrl, setVerifyUrl] = useState("");
  const [error, setError] = useState("");
  const [models, setModels] = useState(0);
  const [pollStatus, setPollStatus] = useState("");
  const [accountLabel, setAccountLabel] = useState("");
  const pollRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => { if (pollRef.current) clearTimeout(pollRef.current); }, []);

  const startFlow = async () => {
    setStep("waiting");
    setError("");
    setPollStatus("Initiating...");
    try {
      const flow = await api.copilotAuthStart() as any;
      if (flow.error) {
        setError(flow.error);
        setStep("error");
        return;
      }
      setUserCode(flow.user_code);
      setVerifyUrl(flow.verification_uri);
      setPollStatus("Waiting for you to enter the code...");

      // Open browser
      window.open(flow.verification_uri, "_blank");

      // Determine provider name for multi-account support
      const providerName = accountLabel.trim()
        ? `copilot-${accountLabel.trim().toLowerCase().replace(/\s+/g, "-")}`
        : "copilot";

      let attempts = 0;
      let currentInterval = Math.max((flow.interval || 5) * 1000, 5000); // GitHub requires minimum 5s
      const startTime = Date.now();
      const maxDuration = 10 * 60 * 1000; // 10 minutes total timeout

      const poll = async () => {
        attempts++;
        if (Date.now() - startTime > maxDuration) {
          setError("Timed out waiting for authorization (10 minutes). Please try again.");
          setStep("error");
          return;
        }
        try {
          const result = await api.copilotAuthPoll(flow.device_code, providerName);
          setPollStatus(`Poll #${attempts}: ${result.status}${result.raw ? ` | ${result.raw}` : ""}`);
          if (result.status === "complete") {
            setModels(result.models ?? 0);
            setStep("complete");
            onComplete();
            return;
          } else if (result.status === "expired") {
            setError("Device code expired. Please try again.");
            setStep("error");
            return;
          } else if (result.status === "denied") {
            setError("Authorization was denied.");
            setStep("error");
            return;
          } else if (result.status === "error") {
            setError(result.error || "Unknown error from GitHub");
            setStep("error");
            return;
          } else if (result.status === "slow_down") {
            // GitHub says slow down — add 5 seconds per spec
            currentInterval += 5000;
            setPollStatus(`Poll #${attempts}: slowing down (interval now ${currentInterval / 1000}s)`);
          }
          // "pending" / "slow_down" -> schedule next poll
        } catch (e: any) {
          setPollStatus(`Poll #${attempts}: network error (retrying...)`);
        }
        pollRef.current = setTimeout(poll, currentInterval);
      };
      pollRef.current = setTimeout(poll, currentInterval);
    } catch (e: any) {
      setError(e.message || "Failed to start device flow");
      setStep("error");
    }
  };

  if (step === "idle") {
    return (
      <div className="space-y-3">
        <p className="text-xs text-secondary">
          Connect your GitHub Copilot subscription. This gives you free access to GPT-4o, Claude Sonnet 4, o1, and more.
          You can add multiple accounts.
        </p>
        <div>
          <label className="text-xs text-secondary block mb-1">Account label (optional — for multiple accounts)</label>
          <input
            value={accountLabel}
            onChange={e => setAccountLabel(e.target.value)}
            placeholder="e.g. work, personal"
            className="w-full rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm focus:border-brand-500 focus:outline-none"
          />
          <p className="text-xs text-secondary mt-1">
            {accountLabel.trim()
              ? `Will register as: copilot-${accountLabel.trim().toLowerCase().replace(/\s+/g, "-")}`
              : "Leave blank for default \"copilot\" provider"}
          </p>
        </div>
        <button onClick={startFlow} className="btn-primary text-sm w-full">
          Connect GitHub Copilot
        </button>
      </div>
    );
  }

  if (step === "waiting") {
    return (
      <div className="space-y-3 text-center">
        <p className="text-xs text-secondary">Enter this code on GitHub:</p>
        <div className="bg-tertiary rounded-lg p-4">
          <p className="text-2xl font-mono font-bold tracking-widest">{userCode}</p>
        </div>
        <a href={verifyUrl} target="_blank" rel="noopener noreferrer"
          className="text-brand-500 text-sm hover:underline block">
          Open {verifyUrl}
        </a>
        <div className="flex items-center justify-center gap-2 text-xs text-secondary">
          <div className="h-2 w-2 rounded-full bg-amber-500 animate-pulse" />
          Waiting for authorization...
        </div>
        <p className="text-xs text-secondary/60">{pollStatus}</p>
        <button onClick={() => {
          if (pollRef.current) clearTimeout(pollRef.current);
          setStep("idle");
        }} className="btn-ghost text-xs">Cancel</button>
      </div>
    );
  }

  if (step === "complete") {
    return (
      <div className="text-center space-y-2">
        <p className="text-emerald-400 font-medium">Connected!</p>
        <p className="text-xs text-secondary">{models} models available via Copilot</p>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <p className="text-red-400 text-sm">{error}</p>
      <p className="text-xs text-secondary/60">Last poll: {pollStatus}</p>
      <button onClick={() => setStep("idle")} className="btn-ghost text-xs">Try Again</button>
    </div>
  );
}

/* ─── API Key Setup ─── */

function ApiKeySetup({ def, onComplete }: { def: ProviderDef; onComplete: () => void }) {
  const [apiKey, setApiKey] = useState("");
  const [status, setStatus] = useState<"idle" | "testing" | "success" | "error">("idle");
  const [message, setMessage] = useState("");

  const submit = async () => {
    if (!apiKey.trim()) return;
    setStatus("testing");
    setMessage("Connecting...");
    try {
      const result = await api.createProvider(def.id, {
        enabled: true, api_key: apiKey, billing: def.billing,
      }) as any;

      if (result.status === "created") {
        setStatus("success");
        setMessage(`Connected — ${result.models} models discovered`);
        onComplete();
      } else {
        setStatus("error");
        setMessage(result.error || "Failed to create provider");
      }
    } catch (e: any) {
      setStatus("error");
      setMessage(e.message || "Connection failed");
    }
  };

  return (
    <div className="space-y-3">
      <div>
        <label className="text-xs text-secondary block mb-1">API Key {def.envVar && `(or set ${def.envVar})`}</label>
        <div className="flex gap-2">
          <input
            type="password"
            value={apiKey}
            onChange={e => setApiKey(e.target.value)}
            placeholder={`sk-...`}
            className="flex-1 rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm focus:border-brand-500 focus:outline-none"
            onKeyDown={e => e.key === "Enter" && submit()}
          />
          <button
            onClick={submit}
            disabled={!apiKey.trim() || status === "testing"}
            className="btn-primary text-sm px-4"
          >
            {status === "testing" ? "Testing..." : "Connect"}
          </button>
        </div>
      </div>
      {message && (
        <p className={`text-xs ${status === "success" ? "text-emerald-400" : status === "error" ? "text-red-400" : "text-secondary"}`}>
          {message}
        </p>
      )}
    </div>
  );
}

/* ─── Google Setup (API Key + OAuth) ─── */

function GoogleSetup({ def, onComplete }: { def: ProviderDef; onComplete: () => void }) {
  const [mode, setMode] = useState<"choose" | "apikey" | "oauth">("choose");

  if (mode === "apikey") {
    return <ApiKeySetup def={{ ...def, kind: "apikey" }} onComplete={onComplete} />;
  }

  if (mode === "oauth") {
    return <GoogleOAuthWizard onComplete={onComplete} />;
  }

  return (
    <div className="space-y-3">
      <p className="text-xs text-secondary">Choose how to connect Google AI:</p>
      <div className="grid grid-cols-2 gap-2">
        <button
          onClick={() => setMode("apikey")}
          className="p-3 rounded-lg border border-themed hover:border-brand-500 text-left transition-colors"
        >
          <p className="text-sm font-medium">API Key</p>
          <p className="text-xs text-secondary mt-1">Use a Google AI Studio API key</p>
        </button>
        <button
          onClick={() => setMode("oauth")}
          className="p-3 rounded-lg border border-themed hover:border-brand-500 text-left transition-colors"
        >
          <p className="text-sm font-medium">Google Sign-In</p>
          <p className="text-xs text-secondary mt-1">One-click with your Google account</p>
        </button>
      </div>
    </div>
  );
}

function GoogleOAuthWizard({ onComplete }: { onComplete: () => void }) {
  const [step, setStep] = useState<"idle" | "waiting" | "complete" | "error">("idle");
  const [error, setError] = useState("");
  const [models, setModels] = useState(0);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    // Listen for postMessage from the callback page
    const handler = (e: MessageEvent) => {
      if (e.data?.type === "google-auth-complete") {
        setModels(e.data.models ?? 0);
        setStep("complete");
        if (pollRef.current) clearInterval(pollRef.current);
        onComplete();
      }
    };
    window.addEventListener("message", handler);
    return () => {
      window.removeEventListener("message", handler);
      if (pollRef.current) clearInterval(pollRef.current);
    };
  }, [onComplete]);

  const startFlow = async () => {
    setStep("waiting");
    setError("");
    try {
      const result = await api.googleAuthStart() as any;

      if (result.error) {
        setError(result.error);
        setStep("error");
        return;
      }

      // Open Google sign-in page — user authorizes, callback handles the rest
      window.open(result.auth_url, "_blank");

      // Also poll the backend in case postMessage doesn't work
      pollRef.current = setInterval(async () => {
        try {
          const poll = await api.googleAuthPoll("");
          if (poll.status === "complete") {
            if (pollRef.current) clearInterval(pollRef.current);
            setModels(poll.models ?? 0);
            setStep("complete");
            onComplete();
          }
        } catch {}
      }, 3000);
    } catch (e: any) {
      setError(e.message || "Failed to start auth");
      setStep("error");
    }
  };

  if (step === "idle") {
    return (
      <div className="space-y-3">
        <p className="text-xs text-secondary">
          Sign in with your Google account to access Gemini 3.1 Pro, 3 Flash, and more — free with your Google subscription.
        </p>
        <button onClick={startFlow} className="btn-primary text-sm w-full">
          Sign in with Google
        </button>
      </div>
    );
  }

  if (step === "waiting") {
    return (
      <div className="space-y-3 text-center">
        <div className="flex items-center justify-center gap-2 text-xs text-secondary">
          <div className="h-2 w-2 rounded-full bg-amber-500 animate-pulse" />
          Waiting for Google sign-in...
        </div>
        <p className="text-xs text-secondary">Complete the sign-in in your browser. This page will update automatically.</p>
        <button onClick={startFlow} className="btn-ghost text-xs">Reopen Sign-in Page</button>
      </div>
    );
  }

  if (step === "complete") {
    return (
      <div className="text-center space-y-2">
        <p className="text-emerald-400 font-medium">Connected!</p>
        <p className="text-xs text-secondary">{models} Gemini models available</p>
      </div>
    );
  }

  return (
    <div className="space-y-3">
      <p className="text-red-400 text-sm">{error}</p>
      <button onClick={() => setStep("idle")} className="btn-ghost text-xs">Try Again</button>
    </div>
  );
}

/* ─── Ollama Setup (local) ─── */

function OllamaSetup({ onComplete }: { onComplete: () => void }) {
  const [endpoint, setEndpoint] = useState("http://localhost:11434");
  const [status, setStatus] = useState<"idle" | "testing" | "success" | "error">("idle");
  const [message, setMessage] = useState("");

  const submit = async () => {
    setStatus("testing");
    try {
      const result = await api.createProvider("ollama", {
        enabled: true, endpoint, billing: "local",
      }) as any;
      if (result.status === "created") {
        setStatus("success");
        setMessage(`Connected — ${result.models} models found`);
        onComplete();
      } else {
        setStatus("error");
        setMessage(result.error || "Failed");
      }
    } catch (e: any) {
      setStatus("error");
      setMessage(e.message || "Cannot connect to Ollama. Is it running?");
    }
  };

  return (
    <div className="space-y-3">
      <p className="text-xs text-secondary">Make sure Ollama is running locally.</p>
      <div className="flex gap-2">
        <input
          value={endpoint}
          onChange={e => setEndpoint(e.target.value)}
          className="flex-1 rounded-lg border border-themed bg-tertiary px-3 py-2 text-sm focus:border-brand-500 focus:outline-none"
        />
        <button onClick={submit} disabled={status === "testing"} className="btn-primary text-sm px-4">
          {status === "testing" ? "Connecting..." : "Connect"}
        </button>
      </div>
      {message && (
        <p className={`text-xs ${status === "success" ? "text-emerald-400" : status === "error" ? "text-red-400" : "text-secondary"}`}>
          {message}
        </p>
      )}
    </div>
  );
}

/* ─── Ollama Model Manager ─── */

function OllamaModelManager() {
  const [models, setModels] = useState<any[]>([]);
  const [loading, setLoading] = useState(true);
  const [toggling, setToggling] = useState<string | null>(null);

  const fetchModels = useCallback(async () => {
    setLoading(true);
    try {
      const data = await api.getOllamaModels() as any;
      setModels(data.models ?? []);
    } catch {}
    setLoading(false);
  }, []);

  useEffect(() => { fetchModels(); }, [fetchModels]);

  const toggle = async (name: string, enabled: boolean) => {
    setToggling(name);
    try {
      await api.toggleOllamaModel(name, !enabled);
      await fetchModels();
    } catch {}
    setToggling(null);
  };

  const formatSize = (bytes: number) => {
    if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
    if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
    return `${bytes} B`;
  };

  return (
    <div>
      <h3 className="text-sm font-medium text-secondary mb-3">Ollama Models</h3>
      <div className="card overflow-hidden p-0">
        {loading ? (
          <div className="p-4 text-center text-secondary text-sm">Loading models...</div>
        ) : models.length === 0 ? (
          <div className="p-4 text-center text-secondary text-sm">
            No models found. Pull one with: <code className="bg-tertiary px-1 rounded">ollama pull llama3.2</code>
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-themed text-left text-xs text-secondary">
                <th className="px-4 py-2">Model</th>
                <th className="px-4 py-2">Family</th>
                <th className="px-4 py-2">Size</th>
                <th className="px-4 py-2">Params</th>
                <th className="px-4 py-2 text-right">Enabled</th>
              </tr>
            </thead>
            <tbody>
              {models.map(m => (
                <tr key={m.name} className="border-b border-themed-faint hover:bg-hover">
                  <td className="px-4 py-2 font-mono text-xs">{m.name}</td>
                  <td className="px-4 py-2 text-secondary">{m.family || "—"}</td>
                  <td className="px-4 py-2 text-secondary">{formatSize(m.size_bytes)}</td>
                  <td className="px-4 py-2 text-secondary">{m.parameter_size || "—"}</td>
                  <td className="px-4 py-2 text-right">
                    <button
                      onClick={() => toggle(m.name, m.enabled)}
                      disabled={toggling === m.name}
                      className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                        m.enabled ? "bg-brand-500" : "bg-tertiary"
                      }`}
                    >
                      <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
                        m.enabled ? "translate-x-4" : "translate-x-0.5"
                      }`} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

/* ─── Shared ─── */

function OfflineMessage() {
  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Provider Configuration</h2>
      <div className="card text-center py-12">
        <p className="text-red-400 text-lg font-medium">Proxy Offline</p>
        <p className="text-secondary mt-2 text-sm">
          Start the proxy with: <code className="bg-tertiary px-2 py-1 rounded">coalesce serve</code>
        </p>
      </div>
    </div>
  );
}

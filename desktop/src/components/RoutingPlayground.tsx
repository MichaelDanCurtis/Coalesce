import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

const DEFAULT_WEIGHTS: Record<string, number> = {
  token_count: 0.15,
  code_presence: 0.12,
  reasoning_markers: 0.15,
  technical_terms: 0.08,
  creative_markers: 0.05,
  simple_indicators: 0.08,
  multi_step: 0.07,
  question_complexity: 0.05,
  imperative_verbs: 0.04,
  constraints: 0.04,
  output_format: 0.04,
  reference_keywords: 0.03,
  negation_keywords: 0.03,
  domain_specific: 0.04,
  agentic_keywords: 0.03,
};

const TIERS = ["simple", "medium", "complex", "reasoning"] as const;

type ModelPin = { model_id: string; providers: string[] };
type PinsMap = Record<string, ModelPin[]>;

function formatDimensionLabel(key: string): string {
  return key.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

export default function RoutingPlayground() {
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState<any>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [weights, setWeights] = useState<Record<string, number>>({ ...DEFAULT_WEIGHTS });
  const [showWeights, setShowWeights] = useState(false);

  // Pin management
  const [pins, setPins] = useState<PinsMap>({});
  const [showPins, setShowPins] = useState(true);
  const [pinSaving, setPinSaving] = useState(false);
  const [allModels, setAllModels] = useState<Array<{ id: string; provider: string; name: string }>>([]);
  const [addingTier, setAddingTier] = useState<string | null>(null);

  // Load pins and available models from server
  const loadPins = useCallback(async () => {
    try {
      const data = await api.getRoutingPins();
      setPins(data.pins ?? {});
    } catch {}
  }, []);

  const loadModels = useCallback(async () => {
    try {
      const data = await api.getModels() as { data?: Array<{ id: string; owned_by?: string }> };
      const models = (data.data ?? []).map(m => ({
        id: m.id,
        provider: m.owned_by ?? "unknown",
        name: m.id,
      }));
      setAllModels(models);
    } catch {}
  }, []);

  useEffect(() => { loadPins(); loadModels(); }, [loadPins, loadModels]);

  const savePins = async (newPins: PinsMap) => {
    setPinSaving(true);
    setPins(newPins);
    try {
      await api.setRoutingPins(newPins);
    } catch {}
    setPinSaving(false);
  };

  const addPin = (tier: string, modelId: string, provider: string) => {
    const tierPins = [...(pins[tier] ?? [])];
    const existing = tierPins.find(p => p.model_id === modelId);
    if (existing) {
      // Add provider to existing pin if not already there
      if (!existing.providers.includes(provider)) {
        existing.providers.push(provider);
      }
    } else {
      tierPins.push({ model_id: modelId, providers: [provider] });
    }
    savePins({ ...pins, [tier]: tierPins });
  };

  const removePin = (tier: string, modelId: string, provider?: string) => {
    const tierPins = [...(pins[tier] ?? [])];
    if (provider) {
      // Remove specific provider from pin
      const pin = tierPins.find(p => p.model_id === modelId);
      if (pin) {
        pin.providers = pin.providers.filter(p => p !== provider);
        if (pin.providers.length === 0) {
          const filtered = tierPins.filter(p => p.model_id !== modelId);
          savePins({ ...pins, [tier]: filtered });
          return;
        }
      }
    } else {
      // Remove entire pin
      const filtered = tierPins.filter(p => p.model_id !== modelId);
      savePins({ ...pins, [tier]: filtered });
      return;
    }
    savePins({ ...pins, [tier]: tierPins });
  };

  const movePin = (tier: string, index: number, direction: -1 | 1) => {
    const tierPins = [...(pins[tier] ?? [])];
    const newIndex = index + direction;
    if (newIndex < 0 || newIndex >= tierPins.length) return;
    [tierPins[index], tierPins[newIndex]] = [tierPins[newIndex], tierPins[index]];
    savePins({ ...pins, [tier]: tierPins });
  };

  const movePinProvider = (tier: string, modelId: string, provIndex: number, direction: -1 | 1) => {
    const tierPins = [...(pins[tier] ?? [])];
    const pin = tierPins.find(p => p.model_id === modelId);
    if (!pin) return;
    const newIdx = provIndex + direction;
    if (newIdx < 0 || newIdx >= pin.providers.length) return;
    [pin.providers[provIndex], pin.providers[newIdx]] = [pin.providers[newIdx], pin.providers[provIndex]];
    savePins({ ...pins, [tier]: tierPins });
  };

  const updateWeight = (key: string, value: number) => {
    setWeights((prev) => ({ ...prev, [key]: value }));
  };

  const resetWeights = () => setWeights({ ...DEFAULT_WEIGHTS });

  const analyze = async () => {
    if (!prompt.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const data = await api.routingPlayground(prompt, weights);
      setResult(data);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to analyze");
    } finally {
      setLoading(false);
    }
  };

  const hasPins = Object.values(pins).some(arr => arr.length > 0);

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">Routing Playground</h2>
      <p className="text-sm text-secondary">
        Paste a prompt to see how the router classifies it, then pin models to control which provider handles each tier.
      </p>

      {/* ─── Model Pins ─── */}
      <div className="card">
        <button
          onClick={() => setShowPins(s => !s)}
          className="flex items-center gap-2 text-sm font-medium w-full text-left"
        >
          <span className="text-xs">{showPins ? "\u25BC" : "\u25B6"}</span>
          Model Pins {hasPins && <span className="text-xs text-secondary">({Object.values(pins).flat().length} active)</span>}
          {pinSaving && <span className="text-xs text-secondary ml-auto">Saving...</span>}
        </button>

        {showPins && (
          <div className="mt-3 space-y-4">
            <p className="text-xs text-secondary">
              Pin models to tiers with provider fallback order. When a request is classified as a tier,
              pinned models are tried first — each provider in order until one succeeds.
            </p>

            {TIERS.map(tier => {
              const tierPins = pins[tier] ?? [];
              return (
                <div key={tier} className="border border-themed rounded-lg p-3">
                  <div className="flex items-center gap-2 mb-2">
                    <span className="text-sm font-medium capitalize" style={{ color: "var(--brand-400)" }}>{tier}</span>
                    <span className="text-xs text-secondary">
                      {tierPins.length === 0 ? "— no pins (uses cost-based ranking)" : `${tierPins.length} pinned`}
                    </span>
                  </div>

                  {tierPins.length > 0 && (
                    <div className="space-y-2">
                      {tierPins.map((pin, i) => (
                        <div key={pin.model_id} className="flex items-start gap-2 bg-surface-alt rounded px-3 py-2">
                          <div className="flex flex-col gap-0.5 mr-1">
                            <button
                              onClick={() => movePin(tier, i, -1)}
                              disabled={i === 0}
                              className="text-xs text-secondary hover:text-primary disabled:opacity-20"
                              title="Move up"
                            >{"\u25B2"}</button>
                            <button
                              onClick={() => movePin(tier, i, 1)}
                              disabled={i === tierPins.length - 1}
                              className="text-xs text-secondary hover:text-primary disabled:opacity-20"
                              title="Move down"
                            >{"\u25BC"}</button>
                          </div>

                          <div className="flex-1">
                            <div className="flex items-center gap-2">
                              <span className="text-xs text-secondary">#{i + 1}</span>
                              <span className="font-mono text-xs font-medium">{pin.model_id}</span>
                              <button
                                onClick={() => removePin(tier, pin.model_id)}
                                className="text-xs text-red-400/60 hover:text-red-400 ml-auto"
                                title="Remove pin"
                              >Remove</button>
                            </div>

                            <div className="flex items-center gap-1 mt-1 flex-wrap">
                              <span className="text-xs text-secondary">Providers:</span>
                              {pin.providers.map((prov, pi) => (
                                <span key={prov} className="inline-flex items-center gap-0.5 bg-tertiary rounded px-1.5 py-0.5 text-xs">
                                  <button
                                    onClick={() => movePinProvider(tier, pin.model_id, pi, -1)}
                                    disabled={pi === 0}
                                    className="text-secondary hover:text-primary disabled:opacity-20"
                                  >{"\u25C0"}</button>
                                  <span className={pi === 0 ? "text-brand-400 font-medium" : "text-secondary"}>{prov}</span>
                                  <button
                                    onClick={() => movePinProvider(tier, pin.model_id, pi, 1)}
                                    disabled={pi === pin.providers.length - 1}
                                    className="text-secondary hover:text-primary disabled:opacity-20"
                                  >{"\u25B6"}</button>
                                  <button
                                    onClick={() => removePin(tier, pin.model_id, prov)}
                                    className="text-red-400/40 hover:text-red-400 ml-0.5"
                                  >{"\u00D7"}</button>
                                </span>
                              ))}
                            </div>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}

                  {/* Add Model to Tier */}
                  {addingTier === tier ? (
                    <AddPinSelector
                      allModels={allModels}
                      existingPins={tierPins}
                      onAdd={(modelId, provider) => { addPin(tier, modelId, provider); }}
                      onClose={() => setAddingTier(null)}
                    />
                  ) : (
                    <button
                      onClick={() => setAddingTier(tier)}
                      className="mt-2 text-xs text-brand-400 hover:text-brand-300"
                    >+ Add model</button>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* ─── Model Equivalences ─── */}
      <ModelEquivalences allModels={allModels} />

      {/* ─── Prompt Analysis ─── */}
      <div className="card space-y-4">
        <textarea
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder="Type or paste a prompt to analyze..."
          className="input-field w-full"
          rows={4}
        />

        {/* Collapsible Dimension Weights */}
        <div>
          <button
            onClick={() => setShowWeights((s) => !s)}
            className="flex items-center gap-2 text-sm text-secondary hover:text-primary transition-colors"
          >
            <span className="text-xs">{showWeights ? "\u25BC" : "\u25B6"}</span>
            Dimension Weights
          </button>

          {showWeights && (
            <div className="mt-3 space-y-2 p-3 rounded-lg bg-surface-alt border border-themed">
              <div className="flex items-center justify-between mb-2">
                <span className="text-xs text-secondary">Adjust scoring weights for each dimension</span>
                <button
                  onClick={resetWeights}
                  className="text-xs text-secondary hover:text-primary transition-colors"
                >
                  Reset Defaults
                </button>
              </div>
              <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
                {Object.entries(weights).map(([key, value]) => (
                  <div key={key} className="flex items-center gap-2">
                    <label className="text-xs text-secondary min-w-[130px] truncate" title={key}>
                      {formatDimensionLabel(key)}
                    </label>
                    <input
                      type="range"
                      min={0}
                      max={0.5}
                      step={0.01}
                      value={value}
                      onChange={(e) => updateWeight(key, parseFloat(e.target.value))}
                      className="flex-1 accent-[var(--brand-500)]"
                    />
                    <span className="text-xs font-mono w-10 text-right">{value.toFixed(2)}</span>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>

        <button
          onClick={analyze}
          disabled={loading || !prompt.trim()}
          className="btn-primary disabled:opacity-50"
        >
          {loading ? "Analyzing..." : "Analyze Route"}
        </button>
      </div>

      {error && (
        <div className="card border-red-900">
          <p className="text-red-400 text-sm">{error}</p>
        </div>
      )}

      {result && (
        <div className="space-y-4">
          {/* Tier Result */}
          <div className="card">
            <div className="flex items-center gap-4">
              <div>
                <p className="text-xs text-secondary">Tier</p>
                <p className="text-2xl font-bold" style={{ color: "var(--brand-400)" }}>{result.tier}</p>
              </div>
              <div>
                <p className="text-xs text-secondary">Score</p>
                <p className="text-2xl font-semibold">{result.score?.toFixed(3)}</p>
              </div>
            </div>
            {result.reasoning && (
              <p className="mt-3 text-sm text-secondary">{result.reasoning}</p>
            )}
          </div>

          {/* Dimension Scores Visualization */}
          {result.scoring?.dimensions && (
            <div className="card">
              <h3 className="text-sm font-medium mb-3">Dimension Scores</h3>
              <div className="space-y-1.5">
                {Object.entries(result.scoring.dimensions as Record<string, number>).map(
                  ([key, score]) => {
                    const pct = Math.min(100, Math.max(0, (score as number) * 100));
                    return (
                      <div key={key} className="flex items-center gap-2">
                        <span className="text-xs text-secondary min-w-[130px] truncate" title={key}>
                          {formatDimensionLabel(key)}
                        </span>
                        <div className="flex-1 h-2 rounded-full bg-tertiary">
                          <div
                            className="h-full rounded-full bg-brand-500/70 transition-all"
                            style={{ width: `${pct}%` }}
                          />
                        </div>
                        <span className="text-xs font-mono w-10 text-right">
                          {(score as number).toFixed(2)}
                        </span>
                      </div>
                    );
                  }
                )}
              </div>
            </div>
          )}

          {/* Candidate Models */}
          {result.candidates?.length > 0 && (
            <div>
              <h3 className="text-sm font-medium text-secondary mb-3">
                Candidate Models {result.candidates.some((c: any) => c.pinned) ? "(pinned models shown first)" : "(ranked by cost)"}
              </h3>
              <div className="card overflow-hidden p-0">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-themed text-left text-xs text-secondary">
                      <th className="px-4 py-2">#</th>
                      <th className="px-4 py-2">Provider</th>
                      <th className="px-4 py-2">Model</th>
                      <th className="px-4 py-2 text-right">Marginal Cost</th>
                      <th className="px-4 py-2 text-center">Pin</th>
                    </tr>
                  </thead>
                  <tbody>
                    {result.candidates.map((c: any, i: number) => {
                      const tier = (result.scoring?.tier ?? result.tier ?? "").toLowerCase();
                      const isPinned = c.pinned;
                      return (
                        <tr
                          key={i}
                          className={`border-b border-themed-faint ${
                            isPinned ? "bg-brand-600/10" : i === 0 ? "bg-brand-600/5" : ""
                          }`}
                        >
                          <td className="px-4 py-2 text-secondary">
                            {isPinned ? (
                              <span className="text-brand-400" title="Pinned">{"\u{1F4CC}"}</span>
                            ) : (
                              i + 1
                            )}
                          </td>
                          <td className="px-4 py-2">{c.provider}</td>
                          <td className="px-4 py-2 font-mono text-xs">{c.model}</td>
                          <td className="px-4 py-2 text-right">
                            <span
                              className={c.is_free ? "text-emerald-400" : "text-amber-400"}
                            >
                              {c.is_free ? "$0 (free)" : `$${c.marginal_cost_usd?.toFixed(6)}`}
                            </span>
                          </td>
                          <td className="px-4 py-2 text-center">
                            {isPinned ? (
                              <button
                                onClick={() => removePin(tier, c.model, c.provider)}
                                className="text-xs text-red-400/60 hover:text-red-400"
                                title="Unpin this provider"
                              >Unpin</button>
                            ) : (
                              <button
                                onClick={() => addPin(tier, c.model, c.provider)}
                                className="text-xs text-brand-400 hover:text-brand-300"
                                title={`Pin ${c.model} via ${c.provider} for ${tier} tier`}
                              >Pin</button>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/* ─── Add Pin Selector ─── */

function AddPinSelector({
  allModels,
  existingPins,
  onAdd,
  onClose,
}: {
  allModels: Array<{ id: string; provider: string; name: string }>;
  existingPins: ModelPin[];
  onAdd: (modelId: string, provider: string) => void;
  onClose: () => void;
}) {
  const [search, setSearch] = useState("");
  const [selectedModel, setSelectedModel] = useState<string | null>(null);

  // Group models by model ID (same model across multiple providers)
  const modelMap = new Map<string, string[]>();
  for (const m of allModels) {
    if (!modelMap.has(m.id)) modelMap.set(m.id, []);
    modelMap.get(m.id)!.push(m.provider);
  }

  const filtered = [...modelMap.entries()].filter(([id]) =>
    !search || id.toLowerCase().includes(search.toLowerCase())
  );

  return (
    <div className="mt-2 border border-themed rounded-lg bg-surface-alt p-3 space-y-2">
      <div className="flex items-center gap-2">
        <input
          value={search}
          onChange={e => setSearch(e.target.value)}
          placeholder="Search models..."
          className="rounded border border-themed bg-tertiary px-2 py-1 text-xs flex-1"
          autoFocus
        />
        <button onClick={onClose} className="text-xs text-secondary hover:text-primary">Cancel</button>
      </div>

      <div className="max-h-48 overflow-y-auto space-y-0.5">
        {filtered.length === 0 ? (
          <p className="text-xs text-secondary py-2 text-center">No models found</p>
        ) : filtered.map(([modelId, providers]) => {
          const alreadyPinned = existingPins.find(p => p.model_id === modelId);
          const isSelected = selectedModel === modelId;

          return (
            <div key={modelId}>
              <button
                onClick={() => setSelectedModel(isSelected ? null : modelId)}
                className={`w-full text-left px-2 py-1.5 rounded text-xs hover:bg-hover flex items-center gap-2 ${
                  isSelected ? "bg-brand-600/10" : ""
                }`}
              >
                <span className="font-mono flex-1">{modelId}</span>
                <span className="text-secondary">{providers.length} provider{providers.length > 1 ? "s" : ""}</span>
                {alreadyPinned && <span className="text-brand-400 text-xs">pinned</span>}
              </button>

              {isSelected && (
                <div className="ml-4 pl-2 border-l border-themed py-1 space-y-1">
                  <p className="text-xs text-secondary mb-1">Select provider to add:</p>
                  {providers.map(prov => {
                    const alreadyHasProvider = alreadyPinned?.providers.includes(prov);
                    return (
                      <button
                        key={prov}
                        onClick={() => { onAdd(modelId, prov); }}
                        disabled={alreadyHasProvider}
                        className={`block w-full text-left px-2 py-1 rounded text-xs ${
                          alreadyHasProvider
                            ? "text-secondary opacity-50 cursor-not-allowed"
                            : "hover:bg-hover text-brand-400"
                        }`}
                      >
                        {prov} {alreadyHasProvider ? "(already added)" : ""}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ─── Model Equivalences ──────────────────────────────────────────────
type EquivalenceMap = Record<string, string[]>;

function ModelEquivalences({ allModels }: { allModels: Array<{ id: string; provider: string; name: string }> }) {
  const [equivalences, setEquivalences] = useState<EquivalenceMap>({});
  const [show, setShow] = useState(false);
  const [saving, setSaving] = useState(false);
  const [addingGroup, setAddingGroup] = useState(false);
  const [newCanonical, setNewCanonical] = useState("");

  useEffect(() => {
    api.getModelEquivalences().then(d => setEquivalences(d.equivalences ?? {})).catch(() => {});
  }, []);

  const save = async (updated: EquivalenceMap) => {
    setSaving(true);
    setEquivalences(updated);
    try { await api.setModelEquivalences(updated); } catch {}
    setSaving(false);
  };

  const addGroup = () => {
    const name = newCanonical.trim();
    if (!name || equivalences[name]) return;
    save({ ...equivalences, [name]: [] });
    setNewCanonical("");
    setAddingGroup(false);
  };

  const removeGroup = (canonical: string) => {
    const updated = { ...equivalences };
    delete updated[canonical];
    save(updated);
  };

  const addAlias = (canonical: string, alias: string) => {
    if (!alias || equivalences[canonical]?.includes(alias)) return;
    save({ ...equivalences, [canonical]: [...(equivalences[canonical] ?? []), alias] });
  };

  const removeAlias = (canonical: string, alias: string) => {
    save({ ...equivalences, [canonical]: (equivalences[canonical] ?? []).filter(a => a !== alias) });
  };

  const groupCount = Object.keys(equivalences).length;

  // Get unique model IDs across all providers for autocomplete
  const uniqueModelIds = [...new Set(allModels.map(m => m.id))].sort();

  return (
    <div className="card">
      <button
        onClick={() => setShow(s => !s)}
        className="flex items-center gap-2 text-sm font-medium w-full text-left"
      >
        <span className="text-xs">{show ? "\u25BC" : "\u25B6"}</span>
        Model Equivalences {groupCount > 0 && <span className="text-xs text-secondary">({groupCount} groups)</span>}
        {saving && <span className="text-xs text-secondary ml-auto">Saving...</span>}
      </button>

      {show && (
        <div className="mt-3 space-y-3">
          <p className="text-xs text-secondary">
            Group equivalent models across providers. The router treats all aliases as the same model
            for pin matching and cost comparison. E.g., "claude-opus-4-6-thinking" on Google =
            "claude-opus-4.6" on Copilot.
          </p>

          {Object.entries(equivalences).map(([canonical, aliases]) => (
            <div key={canonical} className="border border-themed rounded-lg p-3">
              <div className="flex items-center gap-2 mb-2">
                <span className="text-sm font-mono font-medium" style={{ color: "var(--brand-400)" }}>{canonical}</span>
                <span className="text-xs text-secondary">(canonical)</span>
                <button
                  onClick={() => removeGroup(canonical)}
                  className="ml-auto text-xs text-red-400 hover:text-red-300"
                  title="Remove group"
                >&times;</button>
              </div>

              {aliases.length > 0 && (
                <div className="space-y-1 mb-2">
                  {aliases.map(alias => (
                    <div key={alias} className="flex items-center gap-2 bg-surface-alt rounded px-2 py-1">
                      <span className="text-xs font-mono flex-1">{alias}</span>
                      <span className="text-xs text-secondary">
                        {allModels.filter(m => m.id === alias).map(m => m.provider).join(", ")}
                      </span>
                      <button
                        onClick={() => removeAlias(canonical, alias)}
                        className="text-xs text-red-400 hover:text-red-300"
                      >&times;</button>
                    </div>
                  ))}
                </div>
              )}

              <AliasAdder
                modelIds={uniqueModelIds}
                existingAliases={[canonical, ...aliases]}
                onAdd={(alias) => addAlias(canonical, alias)}
              />
            </div>
          ))}

          {addingGroup ? (
            <div className="flex items-center gap-2">
              <input
                value={newCanonical}
                onChange={e => setNewCanonical(e.target.value)}
                onKeyDown={e => e.key === "Enter" && addGroup()}
                placeholder="Canonical model name (e.g., claude-opus-4.6)"
                className="rounded border border-themed bg-tertiary px-2 py-1 text-xs flex-1"
                autoFocus
              />
              <button onClick={addGroup} className="text-xs text-brand-400 hover:text-brand-300">Add</button>
              <button onClick={() => setAddingGroup(false)} className="text-xs text-secondary hover:text-primary">Cancel</button>
            </div>
          ) : (
            <button
              onClick={() => setAddingGroup(true)}
              className="text-xs text-brand-400 hover:text-brand-300"
            >+ Add equivalence group</button>
          )}
        </div>
      )}
    </div>
  );
}

function AliasAdder({
  modelIds,
  existingAliases,
  onAdd,
}: {
  modelIds: string[];
  existingAliases: string[];
  onAdd: (alias: string) => void;
}) {
  const [adding, setAdding] = useState(false);
  const [search, setSearch] = useState("");

  if (!adding) {
    return (
      <button onClick={() => setAdding(true)} className="text-xs text-brand-400 hover:text-brand-300">
        + Add alias
      </button>
    );
  }

  const filtered = modelIds.filter(
    id => !existingAliases.includes(id) && (!search || id.toLowerCase().includes(search.toLowerCase()))
  );

  return (
    <div className="border border-themed rounded bg-surface-alt p-2 space-y-1">
      <div className="flex items-center gap-2">
        <input
          value={search}
          onChange={e => setSearch(e.target.value)}
          placeholder="Search models..."
          className="rounded border border-themed bg-tertiary px-2 py-1 text-xs flex-1"
          autoFocus
        />
        <button onClick={() => setAdding(false)} className="text-xs text-secondary hover:text-primary">Cancel</button>
      </div>
      <div className="max-h-32 overflow-y-auto space-y-0.5">
        {filtered.length === 0 ? (
          <p className="text-xs text-secondary py-1 text-center">No matching models</p>
        ) : filtered.slice(0, 20).map(id => (
          <button
            key={id}
            onClick={() => { onAdd(id); setAdding(false); setSearch(""); }}
            className="block w-full text-left px-2 py-1 rounded text-xs font-mono hover:bg-hover"
          >{id}</button>
        ))}
      </div>
    </div>
  );
}

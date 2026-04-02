import { useState, useCallback } from "react";
import { api } from "../api/client";
import { useApi } from "../hooks/useApi";

interface SystemPrompt {
  scope: string;
  label: string;
  prompt: string;
}

const PRESETS: { name: string; prompt: string }[] = [
  { name: "Coding Assistant", prompt: "You are a skilled software engineer. Write clean, efficient, well-documented code. Follow best practices and explain your reasoning when making design decisions." },
  { name: "Technical Writer", prompt: "You are a technical writer. Produce clear, accurate documentation. Use precise language and organize information logically with appropriate headings and examples." },
  { name: "Data Analyst", prompt: "You are a data analyst. Provide thorough analysis with statistical rigor. Present findings clearly with supporting evidence and actionable recommendations." },
  { name: "Creative Writer", prompt: "You are a creative writer. Craft engaging, original content with vivid language, strong narrative structure, and attention to style and tone." },
  { name: "Concise Responder", prompt: "Be extremely concise. Answer in as few words as possible. Avoid filler, preamble, and unnecessary explanation. Get straight to the point." },
];

const TEMPLATE_VARS = [
  { var: "{{provider}}", desc: "Current provider name" },
  { var: "{{model}}", desc: "Current model ID" },
  { var: "{{tier}}", desc: "Quality tier (Simple/Medium/Complex/Reasoning)" },
  { var: "{{date}}", desc: "Current date (YYYY-MM-DD)" },
];

const STORAGE_KEY = "coalesce-system-prompts";

function loadPrompts(): SystemPrompt[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return JSON.parse(raw);
  } catch {}
  return [];
}

function savePrompts(prompts: SystemPrompt[]) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(prompts));
}

export default function SystemPrompts() {
  const [prompts, setPrompts] = useState<SystemPrompt[]>(loadPrompts);
  const [editing, setEditing] = useState<SystemPrompt | null>(null);
  const [scope, setScope] = useState("global");
  const [label, setLabel] = useState("");
  const [promptText, setPromptText] = useState("");

  const fetchProviders = useCallback(() => api.getProviders(), []);
  const { data: providersData } = useApi(fetchProviders, 30000);
  const providers = (providersData as any)?.providers ?? [];

  const handleSave = () => {
    if (!promptText.trim()) return;
    const entry: SystemPrompt = {
      scope,
      label: label || scope,
      prompt: promptText,
    };

    let updated: SystemPrompt[];
    if (editing) {
      updated = prompts.map((p) =>
        p.scope === editing.scope && p.label === editing.label ? entry : p
      );
    } else {
      updated = [...prompts, entry];
    }

    setPrompts(updated);
    savePrompts(updated);
    setEditing(null);
    setScope("global");
    setLabel("");
    setPromptText("");
  };

  const handleDelete = (p: SystemPrompt) => {
    const updated = prompts.filter((x) => x !== p);
    setPrompts(updated);
    savePrompts(updated);
  };

  const handleEdit = (p: SystemPrompt) => {
    setEditing(p);
    setScope(p.scope);
    setLabel(p.label);
    setPromptText(p.prompt);
  };

  const applyPreset = (preset: { name: string; prompt: string }) => {
    setLabel(preset.name);
    setPromptText(preset.prompt);
  };

  return (
    <div className="space-y-6">
      <h2 className="text-xl font-semibold">System Prompts</h2>
      <p className="text-sm text-secondary">
        Configure system prompts that are prepended to requests. Set global defaults or per-provider/model overrides.
      </p>

      {/* Editor */}
      <div className="card space-y-4">
        <h3 className="text-sm font-medium">{editing ? "Edit Prompt" : "Add New Prompt"}</h3>

        <div className="grid grid-cols-2 gap-4">
          <div>
            <label className="text-xs text-secondary block mb-1">Scope</label>
            <select
              value={scope}
              onChange={(e) => setScope(e.target.value)}
              className="input-field w-full"
            >
              <option value="global">Global (all requests)</option>
              {providers.map((p: any) => (
                <option key={p.name} value={`provider:${p.name}`}>
                  Provider: {p.name}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label className="text-xs text-secondary block mb-1">Label</label>
            <input
              type="text"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="e.g. Coding Assistant"
              className="input-field w-full"
            />
          </div>
        </div>

        <div>
          <label className="text-xs text-secondary block mb-1">System Prompt</label>
          <textarea
            value={promptText}
            onChange={(e) => setPromptText(e.target.value)}
            placeholder="Enter system prompt text..."
            className="input-field w-full"
            rows={4}
          />
        </div>

        {/* Template Variables */}
        <div>
          <label className="text-xs text-secondary block mb-1">Insert variable</label>
          <div className="flex flex-wrap gap-2">
            {TEMPLATE_VARS.map((tv) => (
              <button
                key={tv.var}
                onClick={() => setPromptText((p) => p + tv.var)}
                className="inline-flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-xs transition-colors hover:bg-hover"
                style={{ borderColor: "var(--border)" }}
              >
                <code className="font-mono font-semibold">{tv.var}</code>
                <span className="text-secondary">{tv.desc}</span>
              </button>
            ))}
          </div>
        </div>

        <div className="flex gap-2">
          <button onClick={handleSave} className="btn-primary" disabled={!promptText.trim()}>
            {editing ? "Update" : "Save"}
          </button>
          {editing && (
            <button
              onClick={() => { setEditing(null); setScope("global"); setLabel(""); setPromptText(""); }}
              className="btn-ghost"
            >
              Cancel
            </button>
          )}
        </div>
      </div>

      {/* Presets */}
      <div>
        <h3 className="text-sm font-medium text-secondary mb-3">Presets</h3>
        <div className="flex flex-wrap gap-2">
          {PRESETS.map((preset) => (
            <button
              key={preset.name}
              onClick={() => applyPreset(preset)}
              className="btn-ghost text-xs"
            >
              {preset.name}
            </button>
          ))}
        </div>
      </div>

      {/* Saved Prompts */}
      {prompts.length > 0 && (
        <div>
          <h3 className="text-sm font-medium text-secondary mb-3">Configured Prompts</h3>
          <div className="space-y-2">
            {prompts.map((p, i) => (
              <div key={i} className="card flex items-start justify-between gap-4">
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 mb-1">
                    <span className="font-medium text-sm">{p.label}</span>
                    <span className="badge-green text-xs">{p.scope}</span>
                  </div>
                  <p className="text-xs text-secondary line-clamp-2">{p.prompt}</p>
                </div>
                <div className="flex gap-1 shrink-0">
                  <button onClick={() => handleEdit(p)} className="btn-ghost text-xs">Edit</button>
                  <button onClick={() => handleDelete(p)} className="btn-ghost text-xs text-red-400">Delete</button>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

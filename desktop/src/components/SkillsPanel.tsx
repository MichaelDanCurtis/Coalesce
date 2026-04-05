import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

interface Skill {
  id: string;
  name: string;
  version: string;
  description: string;
  author?: string;
  source: { type: string; path?: string; repo?: string };
  enabled: boolean;
  harnesses: string[];
  content: { type: string; text?: string };
  installed_at: string;
}

const HARNESS_OPTIONS = ["claude-code", "cursor", "windsurf"];

export default function SkillsPanel() {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [showInstall, setShowInstall] = useState(false);

  const fetchSkills = useCallback(async () => {
    try {
      const data = await api.getSkills();
      setSkills((data as any).skills || []);
    } catch {
      setSkills([]);
    }
    setLoading(false);
  }, []);

  useEffect(() => { fetchSkills(); }, [fetchSkills]);

  const handleToggle = async (id: string) => {
    try {
      await api.toggleSkill(id);
      await fetchSkills();
    } catch {}
  };

  const handleRemove = async (id: string) => {
    try {
      await api.removeSkill(id);
      await fetchSkills();
    } catch {}
  };

  const handleToggleHarness = async (id: string, harness: string) => {
    try {
      await api.toggleSkillHarness(id, harness);
      await fetchSkills();
    } catch {}
  };

  const handleInstall = async (skill: any) => {
    try {
      await api.installSkill(skill);
      await fetchSkills();
      setShowInstall(false);
    } catch {}
  };

  const sourceColor = (type: string) => {
    switch (type.toLowerCase()) {
      case "local": return "bg-blue-500/20 text-blue-400";
      case "github": return "bg-purple-500/20 text-purple-400";
      case "builtin": return "bg-emerald-500/20 text-emerald-400";
      default: return "bg-tertiary text-secondary";
    }
  };

  const enabledCount = skills.filter(s => s.enabled).length;
  const sourceTypes = skills.reduce<Record<string, number>>((acc, s) => {
    const t = s.source?.type || "unknown";
    acc[t] = (acc[t] || 0) + 1;
    return acc;
  }, {});

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">Skills</h2>
          <p className="text-sm text-secondary mt-1">
            Reusable prompt skills that can be attached to coding harnesses
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={() => setShowInstall(!showInstall)}
            className="rounded-lg border border-themed px-4 py-2 text-sm hover:border-brand-500/50 transition-colors"
          >
            {showInstall ? "Cancel" : "+ Install Skill"}
          </button>
          <button
            onClick={fetchSkills}
            className="rounded-lg bg-brand-500 px-4 py-2 text-sm font-medium text-white hover:bg-brand-600 transition-colors"
          >
            Refresh
          </button>
        </div>
      </div>

      {showInstall && <InstallSkillForm onSubmit={handleInstall} onCancel={() => setShowInstall(false)} />}

      {/* Stats bar */}
      <div className="grid grid-cols-4 gap-3">
        <div className="card text-center">
          <p className="text-2xl font-bold">{skills.length}</p>
          <p className="text-xs text-secondary">Total Skills</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-emerald-400">{enabledCount}</p>
          <p className="text-xs text-secondary">Enabled</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-brand-400">
            {new Set(skills.flatMap(s => s.harnesses)).size}
          </p>
          <p className="text-xs text-secondary">Harness Coverage</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold">
            {Object.keys(sourceTypes).length}
          </p>
          <p className="text-xs text-secondary">Source Types</p>
        </div>
      </div>

      {loading ? (
        <div className="card text-center py-8">
          <p className="text-sm text-secondary">Loading skills...</p>
        </div>
      ) : skills.length === 0 ? (
        <div className="card text-center py-12">
          <div className="text-4xl mb-3 opacity-30">SK</div>
          <p className="text-sm text-secondary mb-2">No skills installed</p>
          <p className="text-xs text-secondary mb-4">
            Click "+ Install Skill" to add a prompt skill for your coding harnesses.
          </p>
        </div>
      ) : (
        <div className="space-y-3">
          {skills.map((skill) => (
            <div
              key={skill.id}
              className="card cursor-pointer hover:border-brand-500/30 transition-colors"
              onClick={() => setExpanded(expanded === skill.id ? null : skill.id)}
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                  <div>
                    <div className="flex items-center gap-2">
                      <p className="text-sm font-medium">{skill.name}</p>
                      <span className="text-[10px] text-secondary">v{skill.version}</span>
                      <span className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${sourceColor(skill.source?.type || "")}`}>
                        {skill.source?.type || "unknown"}
                      </span>
                    </div>
                    <p className="text-xs text-secondary mt-0.5">
                      {skill.description}
                      {skill.author && ` · by ${skill.author}`}
                    </p>
                  </div>
                </div>
                <div className="flex items-center gap-3">
                  <span className="text-[10px] text-secondary">
                    {skill.harnesses.length} harness{skill.harnesses.length !== 1 ? "es" : ""}
                  </span>
                  <button
                    onClick={(e) => { e.stopPropagation(); handleToggle(skill.id); }}
                    className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                      skill.enabled ? "bg-brand-500" : "bg-tertiary"
                    }`}
                  >
                    <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
                      skill.enabled ? "translate-x-4.5" : "translate-x-0.5"
                    }`} />
                  </button>
                  <button
                    onClick={(e) => { e.stopPropagation(); handleRemove(skill.id); }}
                    className="text-xs text-secondary hover:text-red-400 transition-colors"
                  >
                    Del
                  </button>
                </div>
              </div>

              {expanded === skill.id && (
                <div className="mt-3 border-t border-themed pt-3 space-y-3">
                  <div>
                    <p className="text-xs text-secondary mb-2">Harnesses</p>
                    <div className="flex gap-2">
                      {HARNESS_OPTIONS.map((h) => (
                        <button
                          key={h}
                          onClick={(e) => { e.stopPropagation(); handleToggleHarness(skill.id, h); }}
                          className={`rounded-lg px-3 py-1.5 text-xs transition-colors ${
                            skill.harnesses.includes(h)
                              ? "bg-brand-500/20 text-brand-400 border border-brand-500/30"
                              : "bg-tertiary/50 text-secondary border border-themed hover:border-brand-500/30"
                          }`}
                        >
                          {h}
                        </button>
                      ))}
                    </div>
                  </div>
                  {skill.content?.text && (
                    <div>
                      <p className="text-xs text-secondary mb-1">Content Preview</p>
                      <pre className="rounded-lg bg-tertiary/50 px-3 py-2 text-[11px] text-secondary max-h-32 overflow-auto whitespace-pre-wrap">
                        {skill.content.text.slice(0, 500)}{skill.content.text.length > 500 ? "..." : ""}
                      </pre>
                    </div>
                  )}
                  <div className="flex gap-4 text-[10px] text-secondary">
                    <span>Type: {skill.content?.type || "prompt"}</span>
                    <span>Installed: {new Date(skill.installed_at).toLocaleDateString()}</span>
                    {skill.source?.path && <span>Path: {skill.source.path}</span>}
                    {skill.source?.repo && <span>Repo: {skill.source.repo}</span>}
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function InstallSkillForm({ onSubmit, onCancel }: {
  onSubmit: (skill: any) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [promptText, setPromptText] = useState("");

  const inputClass = "w-full rounded-lg border border-themed bg-primary px-2 py-1.5 text-xs focus:border-brand-500/50 focus:outline-none";

  return (
    <div className="card border-brand-500/30 bg-brand-500/5 space-y-3">
      <h3 className="text-sm font-medium">Install Skill</h3>
      <input
        type="text"
        placeholder="Skill name"
        value={name}
        onChange={e => setName(e.target.value)}
        className={inputClass}
      />
      <input
        type="text"
        placeholder="Description"
        value={description}
        onChange={e => setDescription(e.target.value)}
        className={inputClass}
      />
      <div>
        <label className="text-xs text-secondary mb-1 block">Prompt Content</label>
        <textarea
          placeholder="Enter the skill prompt text..."
          value={promptText}
          onChange={e => setPromptText(e.target.value)}
          rows={5}
          className={inputClass + " resize-y"}
        />
      </div>
      <div className="flex gap-2">
        <button
          onClick={() => {
            if (!name.trim() || !promptText.trim()) return;
            onSubmit({
              name: name.trim(),
              description: description.trim(),
              content: { type: "prompt", text: promptText.trim() },
              source: { type: "local" },
              harnesses: ["claude-code"],
            });
          }}
          className="rounded-lg bg-brand-500 px-3 py-1.5 text-xs text-white hover:bg-brand-600 transition-colors"
        >
          Install
        </button>
        <button onClick={onCancel} className="rounded-lg border border-themed px-3 py-1.5 text-xs hover:border-brand-500/50 transition-colors">
          Cancel
        </button>
      </div>
    </div>
  );
}

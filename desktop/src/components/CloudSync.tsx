import { useState, useEffect, useCallback } from "react";
import { api } from "../api/client";

interface SyncStatus {
  enabled: boolean;
  method: string;
  last_sync?: string;
  device_id: string;
  files_synced: number;
  conflicts: Array<{ file: string; message?: string }>;
}

interface SyncConfig {
  method: string;
  auto_sync: boolean;
  interval_secs: number;
  local?: { directory: string };
  git?: { repo_url: string; branch: string };
  webdav?: { url: string; username: string; password: string };
}

type SyncMethod = "local" | "git" | "webdav";

export default function CloudSync() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [, setConfig] = useState<SyncConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [syncing, setSyncing] = useState(false);
  const [saving, setSaving] = useState(false);

  // Editable form state
  const [method, setMethod] = useState<SyncMethod>("local");
  const [autoSync, setAutoSync] = useState(false);
  const [interval, setInterval_] = useState(300);
  const [localDir, setLocalDir] = useState("");
  const [gitRepo, setGitRepo] = useState("");
  const [gitBranch, setGitBranch] = useState("main");
  const [webdavUrl, setWebdavUrl] = useState("");
  const [webdavUser, setWebdavUser] = useState("");
  const [webdavPass, setWebdavPass] = useState("");

  const fetchData = useCallback(async () => {
    try {
      const [statusData, configData] = await Promise.all([
        api.getSyncStatus(),
        api.getSyncConfig(),
      ]);
      setStatus(statusData as SyncStatus);
      const cfg = configData as SyncConfig;
      setConfig(cfg);
      // Populate form from config
      if (cfg) {
        setMethod((cfg.method || "local") as SyncMethod);
        setAutoSync(cfg.auto_sync ?? false);
        setInterval_(cfg.interval_secs ?? 300);
        setLocalDir(cfg.local?.directory || "");
        setGitRepo(cfg.git?.repo_url || "");
        setGitBranch(cfg.git?.branch || "main");
        setWebdavUrl(cfg.webdav?.url || "");
        setWebdavUser(cfg.webdav?.username || "");
        setWebdavPass(cfg.webdav?.password || "");
      }
    } catch {
      setStatus(null);
      setConfig(null);
    }
    setLoading(false);
  }, []);

  useEffect(() => { fetchData(); }, [fetchData]);

  const handleSyncNow = async () => {
    setSyncing(true);
    try {
      await api.syncNow();
      await fetchData();
    } catch {}
    setSyncing(false);
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      const payload: any = {
        method,
        auto_sync: autoSync,
        interval_secs: interval,
      };
      if (method === "local") payload.local = { directory: localDir };
      if (method === "git") payload.git = { repo_url: gitRepo, branch: gitBranch };
      if (method === "webdav") payload.webdav = { url: webdavUrl, username: webdavUser, password: webdavPass };
      await api.updateSyncConfig(payload);
      await fetchData();
    } catch {}
    setSaving(false);
  };

  const inputClass = "w-full rounded-lg border border-themed bg-primary px-2 py-1.5 text-xs focus:border-brand-500/50 focus:outline-none";
  const selectClass = inputClass;

  if (loading) {
    return (
      <div className="space-y-6">
        <h2 className="text-xl font-semibold">Cloud Sync</h2>
        <div className="card text-center py-8">
          <p className="text-sm text-secondary">Loading sync status...</p>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-xl font-semibold">Cloud Sync</h2>
          <p className="text-sm text-secondary mt-1">
            Synchronize configuration and skills across devices
          </p>
        </div>
        <div className="flex gap-2">
          <button
            onClick={fetchData}
            className="rounded-lg border border-themed px-4 py-2 text-sm hover:border-brand-500/50 transition-colors"
          >
            Refresh
          </button>
          <button
            onClick={handleSyncNow}
            disabled={syncing}
            className="rounded-lg bg-brand-500 px-4 py-2 text-sm font-medium text-white hover:bg-brand-600 transition-colors disabled:opacity-50"
          >
            {syncing ? "Syncing..." : "Sync Now"}
          </button>
        </div>
      </div>

      {/* Status card */}
      <div className="grid grid-cols-4 gap-3">
        <div className="card text-center">
          <p className="text-2xl font-bold">
            <span className={`inline-block h-3 w-3 rounded-full mr-1.5 ${status?.enabled ? "bg-emerald-400" : "bg-gray-400"}`} />
            {status?.enabled ? "On" : "Off"}
          </p>
          <p className="text-xs text-secondary">Sync Status</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-brand-400">{status?.method || "none"}</p>
          <p className="text-xs text-secondary">Method</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold text-emerald-400">{status?.files_synced ?? 0}</p>
          <p className="text-xs text-secondary">Files Synced</p>
        </div>
        <div className="card text-center">
          <p className="text-2xl font-bold">{status?.conflicts?.length ?? 0}</p>
          <p className="text-xs text-secondary">Conflicts</p>
        </div>
      </div>

      {/* Info row */}
      {status && (
        <div className="card bg-brand-500/5 border-brand-500/20">
          <div className="flex gap-6 text-xs text-secondary">
            <span>Device ID: <span className="font-mono text-primary">{status.device_id}</span></span>
            {status.last_sync && (
              <span>Last sync: <span className="text-primary">{new Date(status.last_sync).toLocaleString()}</span></span>
            )}
          </div>
        </div>
      )}

      {/* Configuration form */}
      <div className="card space-y-4">
        <h3 className="text-sm font-medium">Sync Configuration</h3>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="text-xs text-secondary mb-1 block">Method</label>
            <select value={method} onChange={e => setMethod(e.target.value as SyncMethod)} className={selectClass}>
              <option value="local">Local Directory</option>
              <option value="git">Git Repository</option>
              <option value="webdav">WebDAV</option>
            </select>
          </div>
          <div>
            <label className="text-xs text-secondary mb-1 block">Sync Interval (seconds)</label>
            <input
              type="number"
              min={30}
              value={interval}
              onChange={e => setInterval_(Number(e.target.value))}
              className={inputClass}
            />
          </div>
        </div>

        {method === "local" && (
          <div>
            <label className="text-xs text-secondary mb-1 block">Directory Path</label>
            <input
              type="text"
              placeholder="e.g. ~/Dropbox/coalesce-sync"
              value={localDir}
              onChange={e => setLocalDir(e.target.value)}
              className={inputClass}
            />
          </div>
        )}

        {method === "git" && (
          <div className="grid grid-cols-2 gap-3">
            <div>
              <label className="text-xs text-secondary mb-1 block">Repository URL</label>
              <input
                type="text"
                placeholder="https://github.com/user/coalesce-config.git"
                value={gitRepo}
                onChange={e => setGitRepo(e.target.value)}
                className={inputClass}
              />
            </div>
            <div>
              <label className="text-xs text-secondary mb-1 block">Branch</label>
              <input
                type="text"
                placeholder="main"
                value={gitBranch}
                onChange={e => setGitBranch(e.target.value)}
                className={inputClass}
              />
            </div>
          </div>
        )}

        {method === "webdav" && (
          <div className="space-y-3">
            <div>
              <label className="text-xs text-secondary mb-1 block">WebDAV URL</label>
              <input
                type="text"
                placeholder="https://dav.example.com/coalesce/"
                value={webdavUrl}
                onChange={e => setWebdavUrl(e.target.value)}
                className={inputClass}
              />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="text-xs text-secondary mb-1 block">Username</label>
                <input
                  type="text"
                  placeholder="Username"
                  value={webdavUser}
                  onChange={e => setWebdavUser(e.target.value)}
                  className={inputClass}
                />
              </div>
              <div>
                <label className="text-xs text-secondary mb-1 block">Password</label>
                <input
                  type="password"
                  placeholder="Password"
                  value={webdavPass}
                  onChange={e => setWebdavPass(e.target.value)}
                  className={inputClass}
                />
              </div>
            </div>
          </div>
        )}

        <div className="flex items-center justify-between pt-2 border-t border-themed">
          <div className="flex items-center gap-3">
            <button
              onClick={() => setAutoSync(!autoSync)}
              className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors ${
                autoSync ? "bg-brand-500" : "bg-tertiary"
              }`}
            >
              <span className={`inline-block h-3.5 w-3.5 rounded-full bg-white transition-transform ${
                autoSync ? "translate-x-4.5" : "translate-x-0.5"
              }`} />
            </button>
            <span className="text-xs text-secondary">Auto-sync enabled</span>
          </div>
          <button
            onClick={handleSave}
            disabled={saving}
            className="rounded-lg bg-brand-500 px-4 py-1.5 text-xs text-white hover:bg-brand-600 transition-colors disabled:opacity-50"
          >
            {saving ? "Saving..." : "Save Configuration"}
          </button>
        </div>
      </div>

      {/* Conflicts */}
      {status && status.conflicts.length > 0 && (
        <div className="card space-y-3">
          <h3 className="text-sm font-medium text-amber-400">Conflicts ({status.conflicts.length})</h3>
          <div className="space-y-2">
            {status.conflicts.map((c, i) => (
              <div key={i} className="rounded-lg bg-amber-500/10 border border-amber-500/20 px-3 py-2">
                <p className="text-xs font-medium">{c.file}</p>
                {c.message && <p className="text-[10px] text-secondary mt-0.5">{c.message}</p>}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

use super::types::*;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Engine that synchronizes local config/skills to a sync target.
pub struct SyncEngine {
    config: RwLock<SyncConfig>,
    config_path: PathBuf,
    data_base_dir: PathBuf,
}

impl SyncEngine {
    /// Create a new sync engine using the default config directory.
    pub fn new() -> Self {
        let base = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("coalesce");
        Self::with_base_dir(base)
    }

    /// Create a sync engine with a custom base directory (for tests).
    pub fn with_base_dir(base: PathBuf) -> Self {
        let config_path = base.join("sync.json");
        std::fs::create_dir_all(&base).ok();

        let config = if config_path.exists() {
            std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            SyncConfig::default()
        };

        Self {
            config: RwLock::new(config),
            config_path,
            data_base_dir: base,
        }
    }

    /// Get the current sync status.
    pub fn status(&self) -> SyncStatus {
        let config = self.config.read().unwrap();
        let method_name = match &config.method {
            SyncMethod::Git { .. } => "git",
            SyncMethod::WebDav { .. } => "webdav",
            SyncMethod::LocalDir { .. } => "local",
        };
        SyncStatus {
            enabled: config.enabled,
            method: method_name.to_string(),
            last_sync: config.last_sync.clone(),
            device_id: config.device_id.clone(),
            files_synced: 0,
            conflicts: Vec::new(),
        }
    }

    /// Get the current config.
    pub fn get_config(&self) -> SyncConfig {
        self.config.read().unwrap().clone()
    }

    /// Update sync configuration.
    pub fn configure(&self, new_config: SyncConfig) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&new_config).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, json).map_err(|e| e.to_string())?;
        *self.config.write().unwrap() = new_config;
        Ok(())
    }

    /// Trigger a sync operation now.
    pub fn sync_now(&self) -> Result<SyncStatus, String> {
        let config = self.config.read().unwrap().clone();
        if !config.enabled {
            return Err("sync is not enabled".into());
        }
        match &config.method {
            SyncMethod::LocalDir { path } => self.sync_local(Path::new(path)),
            SyncMethod::Git { .. } => {
                Err("Git sync not yet implemented -- use local directory sync".into())
            }
            SyncMethod::WebDav { .. } => {
                Err("WebDAV sync not yet implemented -- use local directory sync".into())
            }
        }
    }

    /// Sync to/from a local directory.
    fn sync_local(&self, target: &Path) -> Result<SyncStatus, String> {
        std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
        let exported = self.export_to_dir(target)?;
        let imported = self.import_from_dir(target)?;

        // Update last_sync timestamp
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut config = self.config.write().unwrap();
            config.last_sync = Some(now.clone());
            let json = serde_json::to_string_pretty(&*config).unwrap_or_default();
            std::fs::write(&self.config_path, json).ok();
        }

        let config = self.config.read().unwrap();
        Ok(SyncStatus {
            enabled: config.enabled,
            method: "local".to_string(),
            last_sync: Some(now),
            device_id: config.device_id.clone(),
            files_synced: exported + imported,
            conflicts: Vec::new(),
        })
    }

    /// Export syncable data (skills dir) to a target directory.
    fn export_to_dir(&self, target: &Path) -> Result<usize, String> {
        let skills_src = self.data_base_dir.join("skills");
        let skills_dst = target.join("skills");
        if !skills_src.exists() {
            return Ok(0);
        }
        std::fs::create_dir_all(&skills_dst).map_err(|e| e.to_string())?;

        let mut count = 0;
        let entries = std::fs::read_dir(&skills_src).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let dst = skills_dst.join(entry.file_name());
                // Only copy if source is newer
                let should_copy = if dst.exists() {
                    let src_mod = path.metadata().and_then(|m| m.modified()).ok();
                    let dst_mod = dst.metadata().and_then(|m| m.modified()).ok();
                    match (src_mod, dst_mod) {
                        (Some(s), Some(d)) => s > d,
                        _ => true,
                    }
                } else {
                    true
                };
                if should_copy {
                    std::fs::copy(&path, &dst).map_err(|e| e.to_string())?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    /// Import syncable data from a source directory (skills that are not yet local).
    fn import_from_dir(&self, source: &Path) -> Result<usize, String> {
        let skills_src = source.join("skills");
        let skills_dst = self.data_base_dir.join("skills");
        if !skills_src.exists() {
            return Ok(0);
        }
        std::fs::create_dir_all(&skills_dst).map_err(|e| e.to_string())?;

        let mut count = 0;
        let entries = std::fs::read_dir(&skills_src).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                let dst = skills_dst.join(entry.file_name());
                // Only import if remote is newer than local
                let should_copy = if dst.exists() {
                    let src_mod = path.metadata().and_then(|m| m.modified()).ok();
                    let dst_mod = dst.metadata().and_then(|m| m.modified()).ok();
                    match (src_mod, dst_mod) {
                        (Some(s), Some(d)) => s > d,
                        _ => false,
                    }
                } else {
                    true
                };
                if should_copy {
                    std::fs::copy(&path, &dst).map_err(|e| e.to_string())?;
                    count += 1;
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_default() {
        let dir = tempfile::tempdir().unwrap();
        let engine = SyncEngine::with_base_dir(dir.path().to_path_buf());
        let status = engine.status();
        assert!(!status.enabled);
        assert_eq!(status.method, "local");
        assert!(status.last_sync.is_none());
    }

    #[test]
    fn test_configure_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let engine = SyncEngine::with_base_dir(dir.path().to_path_buf());
        let new_config = SyncConfig {
            enabled: true,
            method: SyncMethod::LocalDir {
                path: "/tmp/sync-test".to_string(),
            },
            auto_sync: true,
            sync_interval_secs: 120,
            device_id: "test-device".to_string(),
            last_sync: None,
        };
        engine.configure(new_config).unwrap();
        let status = engine.status();
        assert!(status.enabled);
        assert_eq!(status.device_id, "test-device");

        // Verify persistence
        let engine2 = SyncEngine::with_base_dir(dir.path().to_path_buf());
        let config2 = engine2.get_config();
        assert!(config2.enabled);
        assert_eq!(config2.device_id, "test-device");
    }

    #[test]
    fn test_local_sync_export_import() {
        let base_dir = tempfile::tempdir().unwrap();
        let sync_target = tempfile::tempdir().unwrap();

        // Create a skill file in the source
        let skills_dir = base_dir.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("test-skill.json"),
            r#"{"id":"test-skill","name":"Test","version":"1.0","description":"d","source":{"type":"builtin"},"enabled":true,"harnesses":[],"content":{"type":"prompt","text":"hi"},"installed_at":"2026-01-01T00:00:00Z","updated_at":null}"#,
        )
        .unwrap();

        let engine = SyncEngine::with_base_dir(base_dir.path().to_path_buf());
        let config = SyncConfig {
            enabled: true,
            method: SyncMethod::LocalDir {
                path: sync_target.path().to_string_lossy().to_string(),
            },
            auto_sync: false,
            sync_interval_secs: 300,
            device_id: "test".to_string(),
            last_sync: None,
        };
        engine.configure(config).unwrap();

        let status = engine.sync_now().unwrap();
        assert!(status.files_synced > 0);
        assert!(status.last_sync.is_some());
        // Skill should exist in target
        assert!(sync_target.path().join("skills/test-skill.json").exists());
    }

    #[test]
    fn test_sync_disabled_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let engine = SyncEngine::with_base_dir(dir.path().to_path_buf());
        let result = engine.sync_now();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not enabled"));
    }
}

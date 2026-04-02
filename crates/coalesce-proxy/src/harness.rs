//! Harness Configuration — detect, configure, and restore external AI coding tools
//! to route through the Coalesce proxy.
//!
//! Supported harnesses:
//! - Claude Code: sets ANTHROPIC_BASE_URL in ~/.claude/settings.json
//! - Codex: modifies ~/.codex/config.json provider config
//! - OpenCode: modifies ~/.config/opencode/opencode.json provider config

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Status of a single harness (AI coding tool)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatus {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub installed: bool,
    pub configured: bool,
    pub config_path: Option<String>,
    pub backup_exists: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessResult {
    pub success: bool,
    pub message: String,
    pub harness_id: String,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("~"))
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

pub fn detect_all(proxy_url: &str) -> Vec<HarnessStatus> {
    vec![
        detect_claude_code(proxy_url),
        detect_codex(proxy_url),
        detect_opencode(proxy_url),
    ]
}

fn detect_claude_code(proxy_url: &str) -> HarnessStatus {
    let config_path = home_dir().join(".claude").join("settings.json");
    let installed = home_dir().join(".claude").exists();
    let backup_path = config_path.with_extension("json.coalesce-backup");
    let configured = is_claude_code_configured(&config_path, proxy_url);

    HarnessStatus {
        id: "claude-code".into(),
        name: "Claude Code".into(),
        icon: "CC".into(),
        installed,
        configured,
        config_path: Some(config_path.display().to_string()),
        backup_exists: backup_path.exists(),
        description: "Anthropic's AI coding assistant CLI".into(),
    }
}

fn detect_codex(proxy_url: &str) -> HarnessStatus {
    let config_path = home_dir().join(".codex").join("config.json");
    let installed = home_dir().join(".codex").exists();
    let backup_path = config_path.with_extension("json.coalesce-backup");
    let configured = is_codex_configured(&config_path, proxy_url);

    HarnessStatus {
        id: "codex".into(),
        name: "Codex CLI".into(),
        icon: "CX".into(),
        installed,
        configured,
        config_path: Some(config_path.display().to_string()),
        backup_exists: backup_path.exists(),
        description: "OpenAI's coding agent CLI".into(),
    }
}

fn detect_opencode(proxy_url: &str) -> HarnessStatus {
    let config_path = home_dir()
        .join(".config")
        .join("opencode")
        .join("opencode.json");
    let alt_config = home_dir().join(".opencode").join("opencode.json");
    let installed = config_path.parent().map_or(false, |p| p.exists())
        || alt_config.parent().map_or(false, |p| p.exists());
    let actual_path = if config_path.exists() {
        config_path.clone()
    } else {
        alt_config.clone()
    };
    let backup_path = actual_path.with_extension("json.coalesce-backup");
    let configured = is_opencode_configured(&actual_path, proxy_url);

    HarnessStatus {
        id: "opencode".into(),
        name: "OpenCode".into(),
        icon: "OC".into(),
        installed,
        configured,
        config_path: Some(actual_path.display().to_string()),
        backup_exists: backup_path.exists(),
        description: "Open-source AI coding assistant".into(),
    }
}

// ---------------------------------------------------------------------------
// Configuration checks
// ---------------------------------------------------------------------------

fn is_claude_code_configured(path: &PathBuf, proxy_url: &str) -> bool {
    read_json(path)
        .and_then(|v| {
            v.get("env")?
                .get("ANTHROPIC_BASE_URL")?
                .as_str()
                .map(|s| s.starts_with(proxy_url))
        })
        .unwrap_or(false)
}

fn is_codex_configured(path: &PathBuf, proxy_url: &str) -> bool {
    read_json(path)
        .and_then(|v| {
            // Check if any provider has our proxy URL
            let providers = v.get("providers")?.as_object()?;
            Some(providers.values().any(|p| {
                p.get("base_url")
                    .or_else(|| p.get("baseUrl"))
                    .and_then(|u| u.as_str())
                    .map(|s| s.starts_with(proxy_url))
                    .unwrap_or(false)
            }))
        })
        .unwrap_or(false)
}

fn is_opencode_configured(path: &PathBuf, proxy_url: &str) -> bool {
    read_json(path)
        .and_then(|v| {
            let providers = v.get("providers")?.as_object()?;
            Some(providers.values().any(|p| {
                p.get("baseURL")
                    .or_else(|| p.get("baseUrl"))
                    .and_then(|u| u.as_str())
                    .map(|s| s.starts_with(proxy_url))
                    .unwrap_or(false)
            }))
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Configure
// ---------------------------------------------------------------------------

pub fn configure_harness(id: &str, proxy_url: &str) -> HarnessResult {
    match id {
        "claude-code" => configure_claude_code(proxy_url),
        "codex" => configure_codex(proxy_url),
        "opencode" => configure_opencode(proxy_url),
        _ => HarnessResult {
            success: false,
            message: format!("Unknown harness: {}", id),
            harness_id: id.into(),
        },
    }
}

fn configure_claude_code(proxy_url: &str) -> HarnessResult {
    let dir = home_dir().join(".claude");
    let config_path = dir.join("settings.json");
    let harness_id = "claude-code".to_string();

    // Ensure directory exists
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return HarnessResult {
            success: false,
            message: format!("Failed to create ~/.claude: {}", e),
            harness_id,
        };
    }

    // Backup existing config
    if config_path.exists() {
        let backup = config_path.with_extension("json.coalesce-backup");
        if let Err(e) = std::fs::copy(&config_path, &backup) {
            return HarnessResult {
                success: false,
                message: format!("Failed to backup config: {}", e),
                harness_id,
            };
        }
    }

    // Read or create settings
    let mut settings = read_json(&config_path).unwrap_or_else(|| serde_json::json!({}));
    let obj = settings.as_object_mut().unwrap();

    // Set env.ANTHROPIC_BASE_URL to proxy
    let env = obj
        .entry("env")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut();
    if let Some(env) = env {
        env.insert(
            "ANTHROPIC_BASE_URL".into(),
            serde_json::Value::String(proxy_url.to_string()),
        );
    }

    match write_json(&config_path, &settings) {
        Ok(()) => HarnessResult {
            success: true,
            message: format!(
                "Claude Code configured to use Coalesce proxy at {}",
                proxy_url
            ),
            harness_id,
        },
        Err(e) => HarnessResult {
            success: false,
            message: format!("Failed to write config: {}", e),
            harness_id,
        },
    }
}

fn configure_codex(proxy_url: &str) -> HarnessResult {
    let dir = home_dir().join(".codex");
    let config_path = dir.join("config.json");
    let harness_id = "codex".to_string();

    if let Err(e) = std::fs::create_dir_all(&dir) {
        return HarnessResult {
            success: false,
            message: format!("Failed to create ~/.codex: {}", e),
            harness_id,
        };
    }

    // Backup
    if config_path.exists() {
        let backup = config_path.with_extension("json.coalesce-backup");
        let _ = std::fs::copy(&config_path, &backup);
    }

    let mut settings = read_json(&config_path).unwrap_or_else(|| serde_json::json!({}));
    let obj = settings.as_object_mut().unwrap();

    // Add coalesce provider
    let providers = obj
        .entry("providers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut();
    if let Some(providers) = providers {
        providers.insert(
            "coalesce".into(),
            serde_json::json!({
                "name": "Coalesce Proxy",
                "base_url": format!("{}/v1", proxy_url),
                "api_key": "coalesce-proxy"
            }),
        );
    }

    // Set default model provider
    obj.insert(
        "model_provider".into(),
        serde_json::Value::String("coalesce".into()),
    );

    match write_json(&config_path, &settings) {
        Ok(()) => HarnessResult {
            success: true,
            message: format!("Codex configured to use Coalesce proxy at {}", proxy_url),
            harness_id,
        },
        Err(e) => HarnessResult {
            success: false,
            message: format!("Failed to write config: {}", e),
            harness_id,
        },
    }
}

fn configure_opencode(proxy_url: &str) -> HarnessResult {
    let dir = home_dir().join(".config").join("opencode");
    let config_path = dir.join("opencode.json");
    let harness_id = "opencode".to_string();

    if let Err(e) = std::fs::create_dir_all(&dir) {
        return HarnessResult {
            success: false,
            message: format!("Failed to create config dir: {}", e),
            harness_id,
        };
    }

    // Backup
    if config_path.exists() {
        let backup = config_path.with_extension("json.coalesce-backup");
        let _ = std::fs::copy(&config_path, &backup);
    }

    let mut settings = read_json(&config_path).unwrap_or_else(|| serde_json::json!({}));
    let obj = settings.as_object_mut().unwrap();

    let providers = obj
        .entry("providers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut();
    if let Some(providers) = providers {
        providers.insert(
            "coalesce".into(),
            serde_json::json!({
                "provider": "@ai-sdk/openai-compatible",
                "baseURL": format!("{}/v1", proxy_url),
                "apiKey": "coalesce-proxy",
                "name": "Coalesce Proxy"
            }),
        );
    }

    match write_json(&config_path, &settings) {
        Ok(()) => HarnessResult {
            success: true,
            message: format!(
                "OpenCode configured to use Coalesce proxy at {}",
                proxy_url
            ),
            harness_id,
        },
        Err(e) => HarnessResult {
            success: false,
            message: format!("Failed to write config: {}", e),
            harness_id,
        },
    }
}

// ---------------------------------------------------------------------------
// Restore
// ---------------------------------------------------------------------------

pub fn restore_harness(id: &str) -> HarnessResult {
    let harness_id = id.to_string();
    let config_path = match id {
        "claude-code" => home_dir().join(".claude").join("settings.json"),
        "codex" => home_dir().join(".codex").join("config.json"),
        "opencode" => home_dir()
            .join(".config")
            .join("opencode")
            .join("opencode.json"),
        _ => {
            return HarnessResult {
                success: false,
                message: format!("Unknown harness: {}", id),
                harness_id,
            }
        }
    };

    let backup = config_path.with_extension("json.coalesce-backup");

    if !backup.exists() {
        // No backup — just remove the Coalesce-specific entries
        return remove_coalesce_config(id, &config_path);
    }

    match std::fs::copy(&backup, &config_path) {
        Ok(_) => {
            let _ = std::fs::remove_file(&backup);
            HarnessResult {
                success: true,
                message: format!("Restored {} from backup", id),
                harness_id,
            }
        }
        Err(e) => HarnessResult {
            success: false,
            message: format!("Failed to restore backup: {}", e),
            harness_id,
        },
    }
}

fn remove_coalesce_config(id: &str, config_path: &PathBuf) -> HarnessResult {
    let harness_id = id.to_string();
    let mut settings = match read_json(config_path) {
        Some(v) => v,
        None => {
            return HarnessResult {
                success: true,
                message: "No config to restore".into(),
                harness_id,
            }
        }
    };

    match id {
        "claude-code" => {
            if let Some(env) = settings.get_mut("env").and_then(|v| v.as_object_mut()) {
                env.remove("ANTHROPIC_BASE_URL");
            }
        }
        "codex" => {
            if let Some(providers) = settings
                .get_mut("providers")
                .and_then(|v| v.as_object_mut())
            {
                providers.remove("coalesce");
            }
            if settings.get("model_provider").and_then(|v| v.as_str()) == Some("coalesce") {
                settings
                    .as_object_mut()
                    .unwrap()
                    .remove("model_provider");
            }
        }
        "opencode" => {
            if let Some(providers) = settings
                .get_mut("providers")
                .and_then(|v| v.as_object_mut())
            {
                providers.remove("coalesce");
            }
        }
        _ => {}
    }

    match write_json(config_path, &settings) {
        Ok(()) => HarnessResult {
            success: true,
            message: format!("Removed Coalesce config from {}", id),
            harness_id,
        },
        Err(e) => HarnessResult {
            success: false,
            message: format!("Failed to update config: {}", e),
            harness_id,
        },
    }
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn read_json(path: &PathBuf) -> Option<serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

fn write_json(path: &PathBuf, value: &serde_json::Value) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::sync::Mutex;

    // Serialize harness tests since they mutate $HOME
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn with_home<F: FnOnce(&std::path::Path)>(f: F) {
        let _guard = HOME_LOCK.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        f(tmp.path());
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        }
    }

    #[test]
    fn test_detect_not_installed() {
        with_home(|_| {
            let statuses = detect_all("http://localhost:8401");
            for s in &statuses {
                assert!(!s.configured);
                assert!(!s.backup_exists);
            }
            let cc = statuses.iter().find(|s| s.id == "claude-code").unwrap();
            assert!(!cc.installed);
        });
    }

    #[test]
    fn test_configure_detect_restore_claude_code() {
        with_home(|home| {
            let proxy_url = "http://localhost:8401";

            // Create pre-existing config
            let dir = home.join(".claude");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();

            // Configure
            let result = configure_harness("claude-code", proxy_url);
            assert!(result.success, "configure failed: {}", result.message);

            // Verify written correctly
            let config: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(home.join(".claude/settings.json")).unwrap()
            ).unwrap();
            assert_eq!(config["env"]["ANTHROPIC_BASE_URL"].as_str().unwrap(), proxy_url);

            // Detection should show configured
            let statuses = detect_all(proxy_url);
            let cc = statuses.iter().find(|s| s.id == "claude-code").unwrap();
            assert!(cc.installed);
            assert!(cc.configured);
            assert!(cc.backup_exists);

            // Restore
            let result = restore_harness("claude-code");
            assert!(result.success);
            let restored: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join("settings.json")).unwrap()
            ).unwrap();
            assert_eq!(restored["theme"], "dark");
            assert!(restored.get("env").is_none());
        });
    }

    #[test]
    fn test_configure_codex() {
        with_home(|home| {
            let result = configure_harness("codex", "http://localhost:8401");
            assert!(result.success);
            let config: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(home.join(".codex/config.json")).unwrap()
            ).unwrap();
            assert!(config["providers"]["coalesce"].is_object());
            assert_eq!(config["model_provider"].as_str().unwrap(), "coalesce");
        });
    }

    #[test]
    fn test_configure_opencode() {
        with_home(|home| {
            let result = configure_harness("opencode", "http://localhost:8401");
            assert!(result.success);
            let config: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(home.join(".config/opencode/opencode.json")).unwrap()
            ).unwrap();
            assert!(config["providers"]["coalesce"].is_object());
        });
    }

    #[test]
    fn test_restore_no_backup_removes_coalesce() {
        with_home(|_| {
            configure_harness("codex", "http://localhost:8401");
            // Delete the backup to test fallback removal path
            let backup = home_dir().join(".codex/config.json.coalesce-backup");
            if backup.exists() { std::fs::remove_file(&backup).unwrap(); }

            let result = restore_harness("codex");
            assert!(result.success);

            let config: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(home_dir().join(".codex/config.json")).unwrap()
            ).unwrap();
            assert!(config.get("providers").and_then(|p| p.get("coalesce")).is_none());
        });
    }

    #[test]
    fn test_unknown_harness() {
        let result = configure_harness("unknown", "http://localhost:8401");
        assert!(!result.success);
        assert!(result.message.contains("Unknown"));

        let result = restore_harness("unknown");
        assert!(!result.success);
    }
}

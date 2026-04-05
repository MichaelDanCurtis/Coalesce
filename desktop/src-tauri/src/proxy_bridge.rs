use tauri::AppHandle;
use tokio::time::{interval, Duration};
use crate::tray::{HealthData, ProviderStatus};

const PROXY_BASE: &str = "http://127.0.0.1:8402";

/// Periodically poll the proxy health endpoint and update tray icon state
pub async fn start_health_polling(app: AppHandle) {
    let mut ticker = interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;
        let health = fetch_health_data().await;
        let _ = crate::tray::update_tray_menu(&app, &health);
    }
}

pub async fn fetch_health_data() -> HealthData {
    // Fetch health endpoint
    let health_ok = match reqwest::get(format!("{}/health", PROXY_BASE)).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    };

    if !health_ok {
        return HealthData {
            status: "offline".to_string(),
            ..Default::default()
        };
    }

    // Fetch providers for status (includes is_disabled field)
    let mut providers = Vec::new();
    let mut disabled_providers = Vec::new();

    if let Ok(resp) = reqwest::get(format!("{}/api/v1/providers", PROXY_BASE)).await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(arr) = json.as_array() {
                for v in arr {
                    if let Some(name) = v.get("name").and_then(|n| n.as_str()) {
                        let is_disabled = v.get("is_disabled").and_then(|d| d.as_bool()).unwrap_or(false);
                        providers.push(ProviderStatus {
                            name: name.to_string(),
                            status: if is_disabled { "disabled".to_string() } else { "ok".to_string() },
                        });
                        if is_disabled {
                            disabled_providers.push(name.to_string());
                        }
                    }
                }
            }
        }
    }

    // Fetch stats for savings/requests
    let (total_requests, total_savings_usd) = match reqwest::get(format!("{}/api/v1/stats", PROXY_BASE)).await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                let reqs = json.get("total_requests")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let free = json.get("total_free_requests")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let savings = free as f64 * 0.003; // estimated per-request savings
                (reqs, savings)
            } else {
                (0, 0.0)
            }
        }
        Err(_) => (0, 0.0),
    };

    // Fetch cache status
    let cache_enabled = match reqwest::get(format!("{}/api/v1/cache/stats", PROXY_BASE)).await {
        Ok(resp) => {
            resp.json::<serde_json::Value>().await.ok()
                .and_then(|j| j.get("enabled").and_then(|v| v.as_bool()))
                .unwrap_or(false)
        }
        Err(_) => false,
    };

    // Fetch mock status
    let mock_enabled = match reqwest::get(format!("{}/api/v1/mock/status", PROXY_BASE)).await {
        Ok(resp) => {
            resp.json::<serde_json::Value>().await.ok()
                .and_then(|j| j.get("enabled").and_then(|v| v.as_bool()))
                .unwrap_or(false)
        }
        Err(_) => false,
    };

    HealthData {
        status: "ok".to_string(),
        providers,
        total_requests,
        total_savings_usd,
        disabled_providers,
        cache_enabled,
        mock_enabled,
    }
}

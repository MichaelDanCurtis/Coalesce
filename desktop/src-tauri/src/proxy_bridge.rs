use tauri::AppHandle;
use tokio::time::{interval, Duration};
use crate::tray::{HealthData, ProviderStatus};

/// Periodically poll the proxy health endpoint and update tray icon state
pub async fn start_health_polling(app: AppHandle) {
    let mut ticker = interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;
        let health = fetch_health_data().await;
        let _ = crate::tray::update_tray_menu(&app, &health);
    }
}

async fn fetch_health_data() -> HealthData {
    // Fetch health endpoint
    let health_ok = match reqwest::get("http://127.0.0.1:8402/health").await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    };

    if !health_ok {
        return HealthData {
            status: "offline".to_string(),
            ..Default::default()
        };
    }

    // Fetch providers for status
    let providers = match reqwest::get("http://127.0.0.1:8402/api/v1/providers").await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(arr) = json.as_array() {
                    arr.iter()
                        .filter_map(|v| {
                            Some(ProviderStatus {
                                name: v.get("name")?.as_str()?.to_string(),
                                status: "ok".to_string(),
                            })
                        })
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            }
        }
        Err(_) => vec![],
    };

    // Fetch stats for savings/requests
    let (total_requests, total_savings_usd) = match reqwest::get("http://127.0.0.1:8402/api/v1/stats").await {
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

    HealthData {
        status: "ok".to_string(),
        providers,
        total_requests,
        total_savings_usd,
    }
}

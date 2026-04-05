use tauri::{
    AppHandle,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    Manager,
};
use serde::Deserialize;

const PROXY_BASE: &str = "http://127.0.0.1:8402";

#[derive(Debug, Deserialize, Default)]
pub struct HealthData {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub providers: Vec<ProviderStatus>,
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default)]
    pub total_savings_usd: f64,
    #[serde(default)]
    pub disabled_providers: Vec<String>,
    #[serde(default)]
    pub cache_enabled: bool,
    #[serde(default)]
    pub mock_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct ProviderStatus {
    pub name: String,
    pub status: String,
}

pub fn create_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    build_tray_menu(app, None)?;
    Ok(())
}

pub fn update_tray_menu(app: &AppHandle, health: &HealthData) -> Result<(), Box<dyn std::error::Error>> {
    // Remove old tray and rebuild with new data
    if let Some(tray) = app.tray_by_id("main-tray") {
        let menu = build_menu(app, Some(health))?;
        tray.set_menu(Some(menu))?;

        // Update tooltip with status
        let tooltip = if health.status == "ok" {
            format!("Coalesce - {} providers | ${:.4} saved", health.providers.len(), health.total_savings_usd)
        } else {
            "Coalesce - Proxy Offline".to_string()
        };
        tray.set_tooltip(Some(&tooltip))?;
    }
    Ok(())
}

fn build_menu(app: &AppHandle, health: Option<&HealthData>) -> Result<tauri::menu::Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let mut builder = MenuBuilder::new(app);

    // Status header
    if let Some(h) = health {
        let status_text = if h.status == "ok" {
            format!("Status: Online ({} providers)", h.providers.len())
        } else {
            "Status: Offline".to_string()
        };
        let status = MenuItemBuilder::with_id("status", &status_text)
            .enabled(false)
            .build(app)?;
        builder = builder.item(&status);

        // Provider list with toggle capability
        for p in &h.providers {
            let is_disabled = h.disabled_providers.contains(&p.name);
            let icon = if is_disabled { "[ ]" } else if p.status == "ok" { "[x]" } else { "[!]" };
            let item = MenuItemBuilder::with_id(
                format!("toggle_{}", p.name),
                format!("  {} {}", icon, p.name),
            )
            .build(app)?;
            builder = builder.item(&item);
        }

        // Savings
        if h.total_savings_usd > 0.0 {
            let sep = PredefinedMenuItem::separator(app)?;
            builder = builder.item(&sep);
            let savings = MenuItemBuilder::with_id("savings", format!("Saved: ${:.4}", h.total_savings_usd))
                .enabled(false)
                .build(app)?;
            builder = builder.item(&savings);
        }

        // Requests
        if h.total_requests > 0 {
            let reqs = MenuItemBuilder::with_id("requests", format!("Requests: {}", h.total_requests))
                .enabled(false)
                .build(app)?;
            builder = builder.item(&reqs);
        }

        let sep = PredefinedMenuItem::separator(app)?;
        builder = builder.item(&sep);

        // Quick actions section
        let cache_label = if h.cache_enabled { "Cache: On" } else { "Cache: Off" };
        let mock_label = if h.mock_enabled { "Mock: On" } else { "Mock: Off" };

        let clear_cache = MenuItemBuilder::with_id("clear_cache", "Clear Cache").build(app)?;
        let toggle_mock = MenuItemBuilder::with_id("toggle_mock", mock_label).build(app)?;
        let cache_status = MenuItemBuilder::with_id("cache_status", cache_label)
            .enabled(false)
            .build(app)?;
        builder = builder.item(&clear_cache);
        builder = builder.item(&toggle_mock);
        builder = builder.item(&cache_status);

        let sep = PredefinedMenuItem::separator(app)?;
        builder = builder.item(&sep);
    }

    let open = MenuItemBuilder::with_id("open", "Open Dashboard").build(app)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Coalesce").build(app)?;

    let menu = builder
        .item(&open)
        .item(&separator)
        .item(&quit)
        .build()?;

    Ok(menu)
}

fn build_tray_menu(app: &AppHandle, health: Option<&HealthData>) -> Result<(), Box<dyn std::error::Error>> {
    let menu = build_menu(app, health)?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("Coalesce - LLM Router")
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref().to_string();
            match id.as_str() {
                "open" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "quit" => {
                    app.exit(0);
                }
                "clear_cache" => {
                    tauri::async_runtime::spawn(async {
                        let client = reqwest::Client::new();
                        let _ = client.post(format!("{}/api/v1/cache/clear", PROXY_BASE))
                            .send()
                            .await;
                    });
                }
                "toggle_mock" => {
                    tauri::async_runtime::spawn(async {
                        let client = reqwest::Client::new();
                        let _ = client.post(format!("{}/api/v1/mock/toggle", PROXY_BASE))
                            .send()
                            .await;
                    });
                }
                _ => {
                    if let Some(provider_name) = id.strip_prefix("toggle_") {
                        let name = provider_name.to_string();
                        let handle = app.clone();
                        tauri::async_runtime::spawn(async move {
                            toggle_provider(&name, &handle).await;
                        });
                    }
                }
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}

/// Toggle a provider on/off by querying current state then flipping it.
async fn toggle_provider(name: &str, app: &AppHandle) {
    let client = reqwest::Client::new();

    // First, check current state from providers API
    let is_currently_disabled = match client
        .get(format!("{}/api/v1/providers", PROXY_BASE))
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.as_array()
                    .and_then(|arr| {
                        arr.iter().find(|p| p.get("name").and_then(|n| n.as_str()) == Some(name))
                    })
                    .and_then(|p| p.get("is_disabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                false
            }
        }
        Err(_) => false,
    };

    // Toggle: if currently disabled, enable it; if enabled, disable it
    let new_enabled = is_currently_disabled;
    let body = serde_json::json!({ "enabled": new_enabled });
    let _ = client
        .post(format!("{}/api/v1/providers/{}/toggle", PROXY_BASE, name))
        .json(&body)
        .send()
        .await;

    // Trigger a tray refresh by fetching health and updating
    let health = crate::proxy_bridge::fetch_health_data().await;
    let _ = crate::tray::update_tray_menu(app, &health);
}

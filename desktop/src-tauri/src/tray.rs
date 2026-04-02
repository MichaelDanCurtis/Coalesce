use tauri::{
    AppHandle,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    Manager,
};
use serde::Deserialize;

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
            format!("AgentPather - {} providers | ${:.4} saved", health.providers.len(), health.total_savings_usd)
        } else {
            "AgentPather - Proxy Offline".to_string()
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

        // Provider list
        for p in &h.providers {
            let icon = if p.status == "ok" { "+" } else { "x" };
            let item = MenuItemBuilder::with_id(
                format!("provider_{}", p.name),
                format!("  {} {}", icon, p.name),
            )
            .enabled(false)
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
    }

    let open = MenuItemBuilder::with_id("open", "Open Dashboard").build(app)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit AgentPather").build(app)?;

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
        .tooltip("AgentPather - LLM Router")
        .on_menu_event(move |app, event| {
            match event.id().as_ref() {
                "open" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                "quit" => {
                    app.exit(0);
                }
                _ => {}
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

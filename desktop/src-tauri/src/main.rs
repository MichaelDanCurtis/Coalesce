#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;

mod commands;
mod proxy_bridge;
mod tray;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Focus existing window when a second instance is launched
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            // Create system tray
            tray::create_tray(app.handle())?;

            // Start proxy bridge (connects to localhost:8402)
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                proxy_bridge::start_health_polling(handle).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_providers,
            commands::get_stats,
            commands::get_models,
            commands::get_health,
            commands::routing_playground,
            commands::get_quotas,
            commands::get_profiles,
            commands::get_timeline,
            commands::get_costs,
            commands::get_harnesses,
            commands::configure_harness,
            commands::restore_harness,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

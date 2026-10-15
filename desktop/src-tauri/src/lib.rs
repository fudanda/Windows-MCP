mod config;
mod manager;
mod platform;
mod protocol;
mod transfer;

use manager::{Manager, Snapshot};
use serde::Serialize;
use tauri::{Manager as _, State};

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    pub detail: String,
}
impl AppError {
    pub fn new(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
        }
    }
    pub fn storage(error: impl std::fmt::Display) -> Self {
        Self::new("storage_failed", error.to_string())
    }
    pub fn connection(error: impl std::fmt::Display) -> Self {
        Self::new("connection_failed", error.to_string())
    }
}
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}
impl std::error::Error for AppError {}

#[tauri::command]
fn get_snapshot(manager: State<'_, Manager>) -> Snapshot {
    manager.snapshot()
}
#[tauri::command]
fn save_config(manager: State<'_, Manager>, config: config::Config) -> Result<Snapshot, AppError> {
    manager.save(config)
}
#[tauri::command]
fn set_language(
    app: tauri::AppHandle,
    manager: State<'_, Manager>,
    language: String,
) -> Result<Snapshot, AppError> {
    let snapshot = manager.language(language)?;
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(if snapshot.config.language == "en" {
            "Windows-MCP Manager"
        } else {
            "Windows-MCP 管理器"
        });
    }
    Ok(snapshot)
}
#[tauri::command]
fn generate_token() -> Result<String, AppError> {
    config::generate_token()
}
#[tauri::command]
async fn start_server(manager: State<'_, Manager>, generation: u64) -> Result<Snapshot, AppError> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.start_if_current(generation))
        .await
        .map_err(|e| AppError::new("process_failed", e.to_string()))?
}
#[tauri::command]
async fn stop_server(manager: State<'_, Manager>) -> Result<Snapshot, AppError> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.stop())
        .await
        .map_err(|e| AppError::new("process_failed", e.to_string()))
}
#[tauri::command]
async fn test_connection(manager: State<'_, Manager>) -> Result<protocol::Probe, AppError> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.test_connection())
        .await
        .map_err(|e| AppError::new("connection_failed", e.to_string()))?
}

#[tauri::command]
fn transfer_snapshot(transfer: State<'_, transfer::TransferManager>) -> transfer::TransferSnapshot {
    transfer.snapshot()
}
#[tauri::command]
async fn transfer_command(
    transfer: State<'_, transfer::TransferManager>,
    command: transfer::Command,
) -> Result<serde_json::Value, AppError> {
    let transfer = transfer.inner().clone();
    tauri::async_runtime::spawn_blocking(move || transfer.command(command, true))
        .await
        .map_err(|e| AppError::new("transfer_io", e.to_string()))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let manager = Manager::new(app.path().app_config_dir()?, app.path().resource_dir()?)?;
            let transfer = transfer::TransferManager::new(
                app.path().app_config_dir()?,
                app.path().download_dir()?.join("Windows-MCP"),
                app.path().resource_dir()?,
            )?;
            manager.set_transfer_bridge(transfer.bridge());
            if manager.snapshot().config.language == "en" {
                if let Some(window) = app.get_webview_window("main") {
                    window.set_title("Windows-MCP Manager")?;
                }
            }
            app.manage(manager);
            app.manage(transfer);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            save_config,
            set_language,
            generate_token,
            start_server,
            stop_server,
            test_connection,
            transfer_snapshot,
            transfer_command
        ])
        .build(tauri::generate_context!())
        .expect("Could not start Windows-MCP Manager");
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app.state::<Manager>().stop();
            app.state::<transfer::TransferManager>().shutdown();
        }
    });
}

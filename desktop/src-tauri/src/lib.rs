mod config;
mod manager;
mod network;
mod platform;
mod protocol;
mod services;
mod transfer;
mod tray;
mod upgrade;

pub use upgrade::command as upgrade_command;

use manager::{Manager, Snapshot};
use serde::Serialize;
use services::Services;
use tauri::{Emitter, Manager as _, State, menu::ContextMenu};

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
    tray::language(&app, &snapshot.config.language)
        .map_err(|e| AppError::new("process_failed", e.to_string()))?;
    Ok(snapshot)
}
#[tauri::command]
fn generate_token() -> Result<String, AppError> {
    config::generate_token()
}
#[tauri::command]
async fn start_server(
    services: State<'_, Services>,
    generation: u64,
) -> Result<Snapshot, AppError> {
    let services = services.inner().clone();
    let ticket = services.ticket();
    tauri::async_runtime::spawn_blocking(move || services.start(generation, ticket, false))
        .await
        .map_err(|e| AppError::new("process_failed", e.to_string()))?
}
#[tauri::command]
async fn stop_server(services: State<'_, Services>) -> Result<Snapshot, AppError> {
    let request = services.request_stop();
    tauri::async_runtime::spawn_blocking(move || request.run(false))
        .await
        .map_err(|e| AppError::new("process_failed", e.to_string()))
}
#[tauri::command]
async fn start_services(
    services: State<'_, Services>,
    generation: u64,
) -> Result<Snapshot, AppError> {
    let services = services.inner().clone();
    let ticket = services.ticket();
    tauri::async_runtime::spawn_blocking(move || services.start(generation, ticket, true))
        .await
        .map_err(|e| AppError::new("process_failed", e.to_string()))?
}
#[tauri::command]
async fn stop_services(services: State<'_, Services>) -> Result<Snapshot, AppError> {
    let request = services.request_stop();
    tauri::async_runtime::spawn_blocking(move || request.run(true))
        .await
        .map_err(|e| AppError::new("process_failed", e.to_string()))
}
#[tauri::command]
async fn open_tray_menu(app: tauri::AppHandle, window: tauri::Window) -> Result<(), AppError> {
    app.state::<tray::TrayMenu>()
        .menu
        .popup(window)
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
    services: State<'_, Services>,
    command: transfer::Command,
) -> Result<serde_json::Value, AppError> {
    let transfer = transfer.inner().clone();
    let services = services.inner().clone();
    let ticket = services.ticket();
    tauri::async_runtime::spawn_blocking(move || {
        if command.action == "start" {
            serde_json::to_value(services.start_files(ticket)?)
                .map_err(|e| AppError::new("transfer_io", e.to_string()))
        } else {
            transfer.command(command, true)
        }
    })
    .await
    .map_err(|e| AppError::new("transfer_io", e.to_string()))?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            tray::show_main(app);
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
            let selected_language = manager.snapshot().config.language;
            app.manage(Services::new(manager.clone(), transfer.clone()));
            app.manage(manager);
            app.manage(transfer);
            tray::init(app.handle(), &selected_language)?;
            upgrade::listen(app.handle().clone())?;
            let args: Vec<_> = std::env::args().collect();
            if let Some(resume) = upgrade::Resume::from_args(&args) {
                let app = app.handle().clone();
                let services = app.state::<Services>().inner().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    if let Err(error) = resume.restore(&services) {
                        let _ = app.emit_to("main", "tray-error", error);
                        tray::show_main(&app);
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main"
                    && window.app_handle().tray_by_id(tray::ID).is_some()
                    && !window.state::<Services>().exiting()
                    && window.hide().is_ok()
                {
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            save_config,
            set_language,
            generate_token,
            start_server,
            stop_server,
            start_services,
            stop_services,
            open_tray_menu,
            test_connection,
            transfer_snapshot,
            transfer_command
        ])
        .build(tauri::generate_context!())
        .expect("Could not start Windows-MCP Manager");
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app.state::<Services>().shutdown();
        }
    });
}

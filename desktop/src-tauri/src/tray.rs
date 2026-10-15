use crate::services::Services;
use tauri::{
    AppHandle, Emitter, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

pub const ID: &str = "windows-mcp";
pub struct TrayMenu {
    pub menu: Menu<tauri::Wry>,
    show: MenuItem<tauri::Wry>,
    start: MenuItem<tauri::Wry>,
    stop: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
pub fn quit(app: &AppHandle) {
    app.state::<Services>().begin_exit();
    app.exit(0);
}
pub fn language(app: &AppHandle, language: &str) -> tauri::Result<()> {
    let Some(menu) = app.try_state::<TrayMenu>() else {
        return Ok(());
    };
    let en = language == "en";
    menu.show
        .set_text(if en { "Show window" } else { "显示主窗口" })?;
    menu.start.set_text(if en {
        "Start MCP and file transfer"
    } else {
        "一键启动 MCP 和文件传输"
    })?;
    menu.stop.set_text(if en {
        "Stop MCP and file transfer"
    } else {
        "一键停止 MCP 和文件传输"
    })?;
    menu.quit.set_text(if en {
        "Quit (stop all services)"
    } else {
        "退出（停止所有服务）"
    })?;
    if let Some(tray) = app.tray_by_id(ID) {
        tray.set_tooltip(Some(if en {
            "Windows-MCP Manager"
        } else {
            "Windows-MCP 管理器"
        }))?;
    }
    Ok(())
}
pub fn init(app: &AppHandle, selected_language: &str) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "tray-show", "显示主窗口", true, None::<&str>)?;
    let start = MenuItem::with_id(
        app,
        "tray-start",
        "一键启动 MCP 和文件传输",
        true,
        None::<&str>,
    )?;
    let stop = MenuItem::with_id(
        app,
        "tray-stop",
        "一键停止 MCP 和文件传输",
        true,
        None::<&str>,
    )?;
    let quit_item =
        MenuItem::with_id(app, "tray-quit", "退出（停止所有服务）", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &PredefinedMenuItem::separator(app)?,
            &start,
            &stop,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id(ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                show_main(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-show" => show_main(app),
            "tray-quit" => quit(app),
            "tray-start" => {
                let services = app.state::<Services>().inner().clone();
                let generation = services.snapshot().generation;
                let ticket = services.ticket();
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let result = services
                        .start(generation, ticket, true)
                        .and_then(|snapshot| snapshot.error.map_or(Ok(()), Err));
                    if let Err(error) = result {
                        let _ = app.emit_to("main", "tray-error", error);
                        show_main(&app);
                    }
                });
            }
            "tray-stop" => {
                let request = app.state::<Services>().request_stop();
                tauri::async_runtime::spawn_blocking(move || {
                    request.run(true);
                });
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayMenu {
        menu,
        show,
        start,
        stop,
        quit: quit_item,
    });
    language(app, selected_language)
}

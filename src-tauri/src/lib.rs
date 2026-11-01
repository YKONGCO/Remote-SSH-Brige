pub mod clients;
mod command_policy;
pub mod database;
pub mod file_tools;
pub mod mcp;
mod mcp_args;
pub mod model;
pub mod safe_remove;
pub mod security;
pub mod ssh;
pub mod state;
pub mod terminal;

#[cfg(feature = "desktop")]
mod desktop {
    use super::*;
    use std::sync::Arc;
    use tauri::{
        menu::{Menu, MenuItem, PredefinedMenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
        Manager,
    };
    fn show_main(app: &tauri::AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
    type Shared<'a> = tauri::State<'a, Arc<state::AppState>>;
    #[tauri::command]
    async fn mcp_clients(state: Shared<'_>) -> Result<Vec<clients::ClientConnection>, String> {
        let status = state.mcp.lock().unwrap().clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            Ok(clients::list(&clients::targets()?, &status))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }
    #[tauri::command]
    async fn configure_mcp_client(
        state: Shared<'_>,
        id: String,
    ) -> Result<Vec<clients::ClientConnection>, String> {
        let _guard = state.service_lock.lock().await;
        let status = state.mcp.lock().unwrap().clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let targets = clients::targets()?;
            let target = targets
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| anyhow::anyhow!("不支持的客户端"))?;
            clients::install(target, &status)?;
            Ok(clients::list(&targets, &status))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
    }
    #[tauri::command]
    fn snapshot(state: Shared<'_>) -> serde_json::Value {
        state.snapshot_with_audits(false)
    }
    #[tauri::command]
    async fn audit_page(
        state: Shared<'_>,
        query: database::AuditQuery,
    ) -> Result<database::AuditPage, String> {
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || state.audit_page(&query))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))
    }

    async fn save_destination(
        window: tauri::WebviewWindow,
        name: String,
    ) -> Result<Option<std::path::PathBuf>, String> {
        use tauri_plugin_dialog::DialogExt;
        tokio::task::spawn_blocking(move || {
            window
                .dialog()
                .file()
                .set_parent(&window)
                .set_file_name(name)
                .blocking_save_file()
                .map(|path| path.into_path().map_err(|e| e.to_string()))
                .transpose()
        })
        .await
        .map_err(|e| e.to_string())?
    }

    #[tauri::command]
    async fn export_audits(
        state: Shared<'_>,
        window: tauri::WebviewWindow,
        query: database::AuditQuery,
    ) -> Result<bool, String> {
        let Some(path) = save_destination(window, "操作记录.json".into()).await? else {
            return Ok(false);
        };
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || state.export_audits(&query, &path))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))?;
        Ok(true)
    }

    #[tauri::command]
    async fn download_file(
        state: Shared<'_>,
        window: tauri::WebviewWindow,
        server_id: String,
        path: String,
        name: String,
    ) -> Result<bool, String> {
        let Some(destination) = save_destination(window, name).await? else {
            return Ok(false);
        };
        let value = state::execute(
            state.inner().clone(),
            model::Operation {
                kind: "download".into(),
                server_id,
                path,
                ..Default::default()
            },
            "desktop",
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            use base64::Engine;
            let data = value["base64"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("下载结果缺少文件内容"))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
            std::fs::write(destination, bytes)?;
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))?;
        Ok(true)
    }
    #[tauri::command]
    fn runtime_settings(state: Shared<'_>) -> model::RuntimeSettings {
        state.settings.lock().unwrap().clone()
    }
    #[tauri::command]
    fn pending_approvals(state: Shared<'_>) -> Vec<security::ApprovalView> {
        state.approvals.list()
    }
    #[tauri::command]
    fn resolve_approval(state: Shared<'_>, id: String, approved: bool) -> Result<(), String> {
        state
            .resolve_approval(&id, approved)
            .map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    fn save_runtime_settings(
        state: Shared<'_>,
        settings: model::RuntimeSettings,
    ) -> Result<model::RuntimeSettings, String> {
        state.save_settings(settings).map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    fn audit_details(state: Shared<'_>, id: String) -> Result<model::Audit, String> {
        state
            .audit_details(&id)
            .map_err(|error| format!("{error:#}"))
    }
    #[tauri::command]
    async fn save_server(
        state: Shared<'_>,
        server: model::Server,
        secret: Option<String>,
    ) -> Result<model::Server, String> {
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || state.save_server(server, secret))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    async fn remove_server(state: Shared<'_>, id: String) -> Result<(), String> {
        let state = state.inner().clone();
        tokio::task::spawn_blocking(move || state.remove_server(&id))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    async fn probe_host(state: Shared<'_>, id: String) -> Result<String, String> {
        let server = state.server(&id).map_err(|e| e.to_string())?;
        let settings = state.settings.lock().unwrap().clone();
        tokio::task::spawn_blocking(move || {
            ssh::fingerprint(&ssh::handshake_with_settings(&server, &settings)?)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    async fn operate(state: Shared<'_>, op: model::Operation) -> Result<serde_json::Value, String> {
        state::execute(state.inner().clone(), op, "desktop")
            .await
            .map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    async fn start_mcp(
        state: Shared<'_>,
        port: u16,
        host: Option<String>,
    ) -> Result<model::McpStatus, String> {
        mcp::start_on(
            state.inner().clone(),
            port,
            host.as_deref().unwrap_or("127.0.0.1"),
        )
        .await
        .map_err(|e| format!("{e:#}"))
    }
    #[tauri::command]
    async fn stop_mcp(state: Shared<'_>) -> Result<(), String> {
        mcp::stop(state.inner().clone())
            .await
            .map_err(|e| format!("{e:#}"))
    }
    pub fn run() {
        tauri::Builder::default()
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                show_main(app);
            }))
            .setup(|app| {
                let shared = state::AppState::open(app.path().app_data_dir()?)?;
                let approval_app = app.handle().clone();
                *shared.approvals.notify.lock().unwrap() =
                    Some(Arc::new(move || show_main(&approval_app)));
                app.manage(shared);
                let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "退出应用", true, None::<&str>)?;
                let separator = PredefinedMenuItem::separator(app)?;
                let menu = Menu::with_items(app, &[&show, &separator, &quit])?;
                let icon = app.default_window_icon().cloned().ok_or("缺少托盘图标")?;
                TrayIconBuilder::with_id("main-tray")
                    .icon(icon)
                    .tooltip(model::MCP_SERVER_TITLE)
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => show_main(app),
                        "quit" => app.exit(0),
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if matches!(
                            event,
                            TrayIconEvent::Click {
                                button: MouseButton::Left,
                                button_state: MouseButtonState::Up,
                                ..
                            }
                        ) {
                            show_main(tray.app_handle());
                        }
                    })
                    .build(app)?;
                Ok(())
            })
            .on_window_event(|window, event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    let close_to_tray = window
                        .state::<Arc<state::AppState>>()
                        .settings
                        .lock()
                        .unwrap()
                        .close_to_tray;
                    api.prevent_close();
                    if close_to_tray {
                        let _ = window.hide();
                    } else {
                        window.app_handle().exit(0);
                    }
                }
            })
            .invoke_handler(tauri::generate_handler![
                snapshot,
                audit_page,
                export_audits,
                download_file,
                mcp_clients,
                configure_mcp_client,
                runtime_settings,
                pending_approvals,
                resolve_approval,
                save_runtime_settings,
                audit_details,
                save_server,
                remove_server,
                probe_host,
                operate,
                start_mcp,
                stop_mcp
            ])
            .run(tauri::generate_context!())
            .expect("启动 Remote SSH Brige 失败");
    }
}
#[cfg(feature = "desktop")]
pub use desktop::run;

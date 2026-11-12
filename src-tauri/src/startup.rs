use crate::{clients, mcp, state::AppState};
use anyhow::Result;
use std::sync::Arc;

pub async fn run(state: Arc<AppState>) {
    run_with_targets(state, clients::targets).await;
}

async fn run_with_targets(
    state: Arc<AppState>,
    targets: impl FnOnce() -> Result<Vec<clients::ClientTarget>> + Send + 'static,
) {
    let settings = state.settings.lock().unwrap().clone();
    if !settings.mcp_auto_start {
        return;
    }
    let result = async {
        let status = mcp::start_on(state.clone(), settings.mcp_port, &settings.mcp_host).await?;
        // Keep the token stable until all client configurations have been refreshed.
        let _guard = state.service_lock.lock().await;
        tokio::task::spawn_blocking(move || clients::refresh_configured(&targets()?, &status))
            .await??;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        // A client write failure leaves the service running and is visible in the UI.
        state.mcp.lock().unwrap().error = Some(format!("自动启动 MCP：{error:#}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{McpStatus, RuntimeSettings};

    fn workspace() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(dir.path().join("data")).unwrap();
        (dir, state)
    }

    #[test]
    fn settings_default_to_auto_start_and_validate_endpoints() {
        let settings: RuntimeSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.mcp_auto_start);
        assert_eq!(settings.mcp_host, "127.0.0.1");
        assert_eq!(settings.mcp_port, 18920);
        for invalid in [
            RuntimeSettings {
                mcp_port: 0,
                ..settings.clone()
            },
            RuntimeSettings {
                mcp_host: "localhost".into(),
                ..settings.clone()
            },
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[tokio::test]
    async fn disabled_startup_does_not_start_or_read_client_files() {
        let (_dir, state) = workspace();
        state.settings.lock().unwrap().mcp_auto_start = false;
        run_with_targets(state.clone(), || panic!("must not inspect clients")).await;
        assert!(!state.mcp.lock().unwrap().running);
        assert!(state.mcp.lock().unwrap().error.is_none());
    }

    #[tokio::test]
    async fn startup_rotates_client_tokens_and_remembers_successful_endpoints() {
        let (dir, state) = workspace();
        let targets = clients::targets_at(&dir.path().join("clients"));
        let old = McpStatus {
            running: true,
            host: "127.0.0.1".into(),
            port: 18920,
            token: "old-token".into(),
            error: None,
        };
        for target in &targets {
            clients::install(target, &old).unwrap();
        }
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        state
            .save_settings(RuntimeSettings {
                mcp_auto_start: true,
                mcp_port: port,
                ..Default::default()
            })
            .unwrap();
        let first_targets = targets.clone();
        run_with_targets(state.clone(), move || Ok(first_targets)).await;
        let first = state.mcp.lock().unwrap().clone();
        assert!(first.running);
        assert!(first.error.is_none());
        assert_eq!(first.port, port);
        assert_ne!(first.token, old.token);
        assert!(clients::list(&targets, &first)
            .iter()
            .all(|c| c.status == "configured"));
        mcp::stop(state.clone()).await.unwrap();
        drop(state);

        let state = AppState::open(dir.path().join("data")).unwrap();
        assert!(state.settings.lock().unwrap().mcp_auto_start);
        assert_eq!(state.settings.lock().unwrap().mcp_port, port);
        let second_targets = targets.clone();
        run_with_targets(state.clone(), move || Ok(second_targets)).await;
        let second = state.mcp.lock().unwrap().clone();
        assert!(second.running);
        assert_ne!(second.token, first.token);
        assert!(clients::list(&targets, &second)
            .iter()
            .all(|c| c.status == "configured"));
        mcp::stop(state).await.unwrap();
    }

    #[tokio::test]
    async fn manual_start_only_remembers_successfully_bound_endpoints() {
        let (dir, state) = workspace();
        state
            .save_settings(RuntimeSettings {
                mcp_auto_start: false,
                ..Default::default()
            })
            .unwrap();
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        mcp::start_on(state.clone(), port, " 0.0.0.0 ")
            .await
            .unwrap();
        mcp::stop(state.clone()).await.unwrap();
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(
            mcp::start(state.clone(), occupied.local_addr().unwrap().port())
                .await
                .is_err()
        );
        drop(state);
        let reopened = AppState::open(dir.path().join("data")).unwrap();
        let settings = reopened.settings.lock().unwrap();
        assert_eq!(settings.mcp_host, "0.0.0.0");
        assert_eq!(settings.mcp_port, port);
        assert!(!settings.mcp_auto_start);
    }

    #[tokio::test]
    async fn occupied_port_reports_failure_without_touching_clients() {
        let (_dir, state) = workspace();
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        state
            .save_settings(RuntimeSettings {
                mcp_auto_start: true,
                mcp_port: reservation.local_addr().unwrap().port(),
                ..Default::default()
            })
            .unwrap();
        run_with_targets(state.clone(), || panic!("must not inspect clients")).await;
        let status = state.mcp.lock().unwrap();
        assert!(!status.running);
        assert!(status.error.as_ref().unwrap().contains("自动启动 MCP"));
    }

    #[tokio::test]
    async fn client_errors_keep_service_running_and_refresh_other_clients() {
        let (dir, state) = workspace();
        let targets = clients::targets_at(&dir.path().join("clients"));
        std::fs::create_dir_all(targets[0].path.parent().unwrap()).unwrap();
        std::fs::write(&targets[0].path, "{invalid").unwrap();
        clients::install(
            &targets[1],
            &McpStatus {
                running: true,
                token: "old-token".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        state
            .save_settings(RuntimeSettings {
                mcp_auto_start: true,
                mcp_port: port,
                ..Default::default()
            })
            .unwrap();
        let startup_targets = targets.clone();
        run_with_targets(state.clone(), move || Ok(startup_targets)).await;
        let status = state.mcp.lock().unwrap().clone();
        assert!(status.running);
        assert!(status.error.as_ref().unwrap().contains("Claude Code"));
        assert_eq!(
            clients::list(&targets[1..2], &status)[0].status,
            "configured"
        );
        mcp::stop(state).await.unwrap();
    }
}

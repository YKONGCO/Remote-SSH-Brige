use remote_desk_lib::{mcp, state::AppState};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or_else(|| {
        anyhow::anyhow!("Usage: remote-desk-headless <data-directory> [port] [listen-ip]")
    })?;
    let port = args.next().unwrap_or_else(|| "18920".into()).parse()?;
    let host = args.next().unwrap_or_else(|| "127.0.0.1".into());
    let state = AppState::open(dir.into())?;
    let status = mcp::start_on(state.clone(), port, &host).await?;
    eprintln!("{}", serde_json::to_string(&status)?);
    tokio::signal::ctrl_c().await?;
    mcp::stop(state).await?;
    Ok(())
}

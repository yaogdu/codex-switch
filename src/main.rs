use codex_switch_poc::{app, Config, ProxyState};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = config_path();
    let config = Config::from_path(&config_path)?;
    let listen = config.listen.clone();
    let state = ProxyState::new(config)?;
    let listener = tokio::net::TcpListener::bind(&listen).await?;

    println!("codex-switch-poc listening on http://{listen}");
    println!("config: {}", config_path.display());

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn config_path() -> PathBuf {
    std::env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .windows(2)
        .find(|args| args[0] == "--config")
        .map(|args| PathBuf::from(&args[1]))
        .or_else(|| std::env::var_os("CODEX_SWITCH_POC_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("poc/config.json"))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

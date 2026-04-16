pub mod audio;
pub mod config;
pub mod hotkey;
pub mod inference;
pub mod ui;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Canario starting up...");

    // TODO: Initialize GTK4 app
    // TODO: Load config
    // TODO: Set up hotkey listener
    // TODO: Download model if needed
    // TODO: Start main loop

    Ok(())
}

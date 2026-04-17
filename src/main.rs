#[cfg(feature = "gui")]
pub mod ui;

pub mod audio;
pub mod config;
pub mod hotkey;
pub mod inference;
pub mod history;

#[cfg(feature = "gui")]
fn main() -> anyhow::Result<()> {
    use ui::app::CanarioApp;

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Canario starting up...");

    let app = CanarioApp::new();
    app.run()
}

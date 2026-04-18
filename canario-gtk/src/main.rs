#[cfg(feature = "gui")]
fn main() -> anyhow::Result<()> {
    use canario_core::Canario;
    use ui::app::CanarioGtkApp;

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("Canario starting up...");

    let (canario, rx) = Canario::new()?;

    // Install desktop files
    if let Err(e) = canario.install_desktop_files() {
        tracing::warn!("Failed to install .desktop file: {}", e);
    }

    let icon_svg = include_bytes!("../../assets/canario.svg");
    if let Err(e) = canario_core::autostart::install_icon(icon_svg) {
        tracing::warn!("Failed to install icon: {}", e);
    }

    let app = CanarioGtkApp::new(canario, rx);
    app.run()
}

pub mod ui;

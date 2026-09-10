#[cfg(feature = "gui")]
fn main() -> anyhow::Result<()> {
    use canario_core::Canario;
    use ui::app::CanarioGtkApp;

    // Initialize logging: daily-rotated, non-blocking log file under the
    // XDG state dir (always) + stderr in debug builds or when RUST_LOG /
    // --verbose is set. The guard must live until exit — dropping it
    // flushes buffered log lines.
    let _log_guard = init_logging();

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

#[cfg(feature = "gui")]
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(canario_core::diagnostics::DEFAULT_LOG_FILTER)
    });

    let stderr_enabled = cfg!(debug_assertions)
        || std::env::var_os("RUST_LOG").is_some()
        || std::env::args().any(|a| a == "--verbose");

    let log_dir = canario_core::diagnostics::log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "⚠  Cannot create log dir {:?}: {} — logging to stderr only",
            log_dir, e
        );
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
        return None;
    }

    let file_appender =
        tracing_appender::rolling::daily(&log_dir, canario_core::diagnostics::LOG_FILE_PREFIX);
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .with(stderr_enabled.then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr)))
        .init();

    Some(guard)
}

pub mod ui;

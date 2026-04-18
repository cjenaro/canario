/// Canario CLI — thin wrapper around canario-core.
use std::io::{self, BufRead, Write};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.contains(&"--toggle-external".to_string()) {
        let socket_path = std::env::temp_dir().join("canario-hotkey.sock");
        if !socket_path.exists() {
            anyhow::bail!("Canario GUI is not running (socket not found)");
        }
        let sock = std::os::unix::net::UnixDatagram::unbound()?;
        sock.send_to(b"toggle", &socket_path)?;
        eprintln!("✅ Toggle command sent");
        return Ok(());
    }

    eprintln!("Canario CLI v0.1.1");
    eprintln!("Usage:");
    eprintln!("  canario-cli --toggle-external  Send toggle to running GUI");
    eprintln!();
    eprintln!("For full CLI functionality (mic streaming, WAV transcription),");
    eprintln!("use canario-core as a library in your own application.");

    Ok(())
}

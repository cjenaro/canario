/// Canario CLI — thin wrapper around canario-core.
use std::io::{self, Write};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Keep the non-blocking writer alive until process exit (dropping it
    // flushes buffered log lines to the file).
    let _log_guard = init_logging(&args);

    if args.contains(&"--toggle-external".to_string()) {
        return cmd_toggle_external();
    }

    if args.contains(&"--download".to_string()) {
        return cmd_download();
    }

    if args.contains(&"--diagnostics".to_string()) {
        return cmd_diagnostics();
    }

    if let Some(pos) = args.iter().position(|a| a == "--wav") {
        let wav_path = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("");
        return cmd_wav(wav_path);
    }

    if args.contains(&"--mic".to_string()) {
        let paste = args.contains(&"--paste".to_string());
        let toggle = args.contains(&"--toggle".to_string());
        return cmd_mic(paste, toggle);
    }

    print_usage();
    Ok(())
}

/// Install the tracing subscriber: a daily-rotated, non-blocking log file
/// under the XDG state dir (always), plus stderr in debug builds or when
/// `RUST_LOG` / `--verbose` is set.
///
/// Returns the non-blocking writer's guard; the caller must keep it alive
/// until exit or buffered log lines are lost.
fn init_logging(args: &[String]) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(canario_core::diagnostics::DEFAULT_LOG_FILTER)
    });

    let stderr_enabled = cfg!(debug_assertions)
        || std::env::var_os("RUST_LOG").is_some()
        || args.iter().any(|a| a == "--verbose");

    let log_dir = canario_core::diagnostics::log_dir();
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        // No log dir → stderr-only fallback so something is still visible.
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

/// Print the diagnostics bundle (same shape as the sidecar's
/// `diagnostics` command) as pretty JSON on stdout.
fn cmd_diagnostics() -> anyhow::Result<()> {
    let diag = canario_core::diagnostics::collect("canario-cli", env!("CARGO_PKG_VERSION"));
    println!("{}", serde_json::to_string_pretty(&diag)?);
    Ok(())
}

fn print_usage() {
    eprintln!("Canario CLI v{}", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  canario-cli --download             Download the ASR model");
    eprintln!("  canario-cli --diagnostics          Print diagnostics bundle (JSON)");
    eprintln!("  canario-cli --wav <file>           Transcribe a WAV file");
    eprintln!("  canario-cli --mic                  Record from mic until Ctrl+C");
    eprintln!("  canario-cli --mic --paste          Record and auto-paste result");
    eprintln!("  canario-cli --mic --toggle         Press Enter to start/stop recording");
    eprintln!("  canario-cli --toggle-external      Send toggle to running GUI");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --paste    Auto-paste transcription into the focused app");
    eprintln!("  --toggle   Enable Enter-key toggle mode (with --mic)");
    eprintln!("  --verbose  Also log to stderr (logs always go to the state dir)");
}

/// Send a toggle command to the running GUI via Unix socket.
fn cmd_toggle_external() -> anyhow::Result<()> {
    // Shared with the hotkey listener in canario-core — do not hardcode a path here.
    let socket_path = canario_core::hotkey_socket_path();
    if !socket_path.exists() {
        anyhow::bail!("Canario GUI is not running (socket not found)");
    }
    let sock = std::os::unix::net::UnixDatagram::unbound()?;
    sock.send_to(b"toggle", &socket_path)?;
    eprintln!("✅ Toggle command sent");
    Ok(())
}

/// Download the ASR model with progress reporting.
fn cmd_download() -> anyhow::Result<()> {
    let (canario, rx) = canario_core::Canario::new()?;
    canario.download_model()?;

    eprintln!("Downloading model...");
    loop {
        match rx.recv() {
            Ok(canario_core::Event::ModelDownloadProgress { progress: p }) => {
                eprint!("\r⬇  Progress: {:.0}%", p * 100.0);
                io::stderr().flush().ok();
            }
            Ok(canario_core::Event::ModelDownloadComplete) => {
                eprintln!("\n✅ Model download complete!");
                return Ok(());
            }
            Ok(canario_core::Event::ModelDownloadFailed { error: e }) => {
                eprintln!("\n❌ Model download failed: {}", e);
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("\n⚠  Event channel closed unexpectedly");
                return Err(anyhow::anyhow!("Event channel closed"));
            }
            _ => {}
        }
    }
}

/// Transcribe a WAV file.
fn cmd_wav(path: &str) -> anyhow::Result<()> {
    if path.is_empty() {
        anyhow::bail!("--wav requires a file path argument");
    }

    let wav_path = std::path::Path::new(path);
    if !wav_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    let (canario, _) = canario_core::Canario::new()?;
    let config = canario.config();
    // Supports custom model paths (ModelVariant::Custom) as well as
    // the built-in downloads.
    let model_paths = config.model_paths()?;

    eprintln!("Loading ASR model...");
    let mut engine = canario_core::TranscriptionEngine::from_paths(model_paths, 4);
    engine.load_model()?;

    eprintln!("Transcribing '{}'...", path);
    let text = engine.transcribe_file(wav_path)?;

    if text.is_empty() {
        eprintln!("(no speech detected)");
    } else {
        println!("{}", text);
    }

    Ok(())
}

/// Record from microphone and transcribe.
fn cmd_mic(paste: bool, toggle: bool) -> anyhow::Result<()> {
    let (canario, rx) = canario_core::Canario::new()?;

    if !canario.is_model_downloaded() {
        anyhow::bail!("Model not downloaded. Run `canario-cli --download` first.");
    }

    // Set up Ctrl+C handler to stop recording
    let canario_stop = canario.clone();
    ctrlc::set_handler(move || {
        eprintln!("\n⏹  Stopping...");
        canario_stop.stop_recording();
    })?;

    if toggle {
        // --mic --toggle: Press Enter to toggle, 'q' to quit
        eprintln!("🎤 Toggle mode: Press Enter to start/stop, 'q' to quit");

        loop {
            eprint!("> ");
            io::stderr().flush().ok();
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            match input.trim() {
                "q" | "quit" | "exit" => {
                    canario.shutdown();
                    eprintln!("👋 Goodbye!");
                    break;
                }
                "" => {
                    // Toggle
                    if canario.is_recording() {
                        canario.stop_recording();
                        eprintln!("⏹  Stopping recording...");
                    } else {
                        match canario.start_recording() {
                            Ok(()) => eprintln!("⏺  Recording... (press Enter to stop)"),
                            Err(e) => eprintln!("❌ Failed to start: {}", e),
                        }
                    }
                }
                _ => {
                    eprintln!("Press Enter to toggle recording, 'q' to quit");
                }
            }

            // Drain any pending events
            drain_events(&rx, &canario, paste);
        }
    } else {
        // --mic: Start recording immediately, stop on Ctrl+C
        eprintln!("🎤 Recording... Press Ctrl+C to stop");
        canario.start_recording()?;

        // Wait for transcription result
        loop {
            match rx.recv() {
                Ok(canario_core::Event::TranscriptionReady { text, .. }) => {
                    if text.is_empty() {
                        eprintln!("(no speech detected)");
                    } else {
                        eprintln!("📝 {}", text);
                        if paste {
                            match canario_core::paste_text(&text) {
                                Ok(pasted) => {
                                    if pasted {
                                        eprintln!("📋 Auto-pasted!");
                                    } else {
                                        eprintln!("📋 Copied to clipboard");
                                    }
                                }
                                Err(e) => eprintln!("⚠  Paste failed: {}", e),
                            }
                        }
                    }
                    // For non-toggle mode, exit after first transcription
                    canario.shutdown();
                    break;
                }
                Ok(canario_core::Event::RecordingStopped) => {
                    // Recording stopped, transcription may follow
                }
                Ok(canario_core::Event::RecordingCancelled) => {
                    eprintln!("\n🚫 Recording cancelled — audio discarded");
                    break;
                }
                Ok(canario_core::Event::Error { message: e }) => {
                    eprintln!("❌ {}", e);
                    break;
                }
                Ok(canario_core::Event::AudioLevel { level }) => {
                    // Show a simple level indicator
                    let bars = (level * 20.0) as usize;
                    eprint!("\r🎤 [{}{}]   ", "█".repeat(bars), "░".repeat(20 - bars));
                    io::stderr().flush().ok();
                }
                Err(_) => {
                    eprintln!("\n⚠  Event channel closed");
                    break;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Drain pending events from the receiver (for toggle mode).
fn drain_events(
    rx: &std::sync::mpsc::Receiver<canario_core::Event>,
    _canario: &canario_core::Canario,
    paste: bool,
) {
    while let Ok(event) = rx.try_recv() {
        match event {
            canario_core::Event::TranscriptionReady { text, .. } => {
                if text.is_empty() {
                    eprintln!("(no speech detected)");
                } else {
                    eprintln!("📝 {}", text);
                    if paste {
                        match canario_core::paste_text(&text) {
                            Ok(pasted) => {
                                if pasted {
                                    eprintln!("📋 Auto-pasted!");
                                } else {
                                    eprintln!("📋 Copied to clipboard");
                                }
                            }
                            Err(e) => eprintln!("⚠  Paste failed: {}", e),
                        }
                    }
                }
            }
            canario_core::Event::RecordingStopped => {
                eprintln!("⏹  Recording stopped");
            }
            canario_core::Event::RecordingCancelled => {
                eprintln!("🚫 Recording cancelled — audio discarded");
            }
            canario_core::Event::Error { message: e } => {
                eprintln!("❌ {}", e);
            }
            _ => {}
        }
    }
}

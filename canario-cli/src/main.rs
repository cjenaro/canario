/// Canario CLI — thin wrapper around canario-core.
use std::io::{self, Write};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.contains(&"--toggle-external".to_string()) {
        return cmd_toggle_external();
    }

    if args.contains(&"--download".to_string()) {
        return cmd_download();
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

fn print_usage() {
    eprintln!("Canario CLI v0.1.1");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  canario-cli --download             Download the ASR model");
    eprintln!("  canario-cli --wav <file>           Transcribe a WAV file");
    eprintln!("  canario-cli --mic                  Record from mic until Ctrl+C");
    eprintln!("  canario-cli --mic --paste          Record and auto-paste result");
    eprintln!("  canario-cli --mic --toggle         Press Enter to start/stop recording");
    eprintln!("  canario-cli --toggle-external      Send toggle to running GUI");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --paste    Auto-paste transcription into the focused app");
    eprintln!("  --toggle   Enable Enter-key toggle mode (with --mic)");
}

/// Send a toggle command to the running GUI via Unix socket.
fn cmd_toggle_external() -> anyhow::Result<()> {
    let socket_path = std::env::temp_dir().join("canario-hotkey.sock");
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
            Ok(canario_core::Event::ModelDownloadProgress(p)) => {
                eprint!("\r⬇  Progress: {:.0}%", p * 100.0);
                io::stderr().flush().ok();
            }
            Ok(canario_core::Event::ModelDownloadComplete) => {
                eprintln!("\n✅ Model download complete!");
                return Ok(());
            }
            Ok(canario_core::Event::ModelDownloadFailed(e)) => {
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
    let model_dir = config.local_model_dir();

    eprintln!("Loading ASR model...");
    let mut engine = canario_core::TranscriptionEngine::new(model_dir, 4);
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
        anyhow::bail!(
            "Model not downloaded. Run `canario-cli --download` first."
        );
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
                Ok(canario_core::Event::Error(e)) => {
                    eprintln!("❌ {}", e);
                    break;
                }
                Ok(canario_core::Event::AudioLevel(level)) => {
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
fn drain_events(rx: &std::sync::mpsc::Receiver<canario_core::Event>, _canario: &canario_core::Canario, paste: bool) {
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
            canario_core::Event::Error(e) => {
                eprintln!("❌ {}", e);
            }
            _ => {}
        }
    }
}

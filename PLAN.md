# Canario Development Plan

> Native Linux voice-to-text using Parakeet TDT, inspired by [Hex](https://github.com/kitlangton/Hex) for macOS.

## What Works (v0.0.1)

- ✅ CLI tool (`canario-cli`) that transcribes WAV files and live microphone audio
- ✅ Downloads INT8 quantized Parakeet TDT v3 model from HuggingFace (~640MB)
- ✅ Uses `sherpa-onnx` Rust bindings for ONNX inference + TDT decoding
- ✅ Audio capture via `cpal`, resampling, WAV reading/writing
- ✅ Skeleton modules for: config, hotkey, inference engine, UI, paste

## Quick Start

```bash
# Download model
./target/release/canario-cli --download

# Transcribe a WAV file
./target/release/canario-cli --wav recording.wav

# Record from microphone (Ctrl+C to stop)
./target/release/canario-cli --mic
```

## Architecture

```
src/
├── main.rs                 # GUI app entry point (placeholder)
├── bin/canario-cli.rs      # ✅ Working CLI prototype
├── audio/mod.rs            # Audio capture (cpal + ring buffer)
├── config/mod.rs           # AppConfig (JSON persistence)
├── hotkey/mod.rs           # Global hotkey (placeholder)
├── inference/mod.rs        # TranscriptionEngine wrapper (placeholder, real logic in CLI)
└── ui/
    ├── mod.rs              # AppState, transcription cycle
    └── paste.rs            # xdotool/wtype text paste
```

### Key Dependencies

| Crate | Purpose |
|-------|---------|
| `sherpa-onnx` | ONNX Runtime inference + TDT decoding + VAD (C++ via FFI) |
| `cpal` | Cross-platform audio capture |
| `gtk4` + `adw` + `relm4` | GTK4 GUI with Adwaita + declarative state management |
| `reqwest` | Download models from HuggingFace |
| `parking_lot` | Fast mutex for shared audio buffer |

### Models

| Model | Source | Size | Languages |
|-------|--------|------|-----------|
| Parakeet TDT v3 (INT8) | `csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` | ~640MB | Multilingual (EN, ES, FR, DE, etc.) |
| Parakeet TDT v2 (INT8) | `csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8` | ~640MB | English only |

Stored at: `~/.local/share/canario/models/sherpa-parakeet-tdt-v3/`

### Reference: How Hex Does It (macOS)

Hex uses:
- **SwiftUI + TCA** (Composable Architecture) for UI
- **FluidAudio** Swift library for Parakeet inference via Core ML
- **AVAudioEngine** for mic capture with ring buffer ("SuperFastCaptureController")
- **Global hotkey** via Sauce framework (macOS accessibility)
- **Auto-paste** via NSPasteboard

Our Linux equivalent:
- **GTK4 + Relm4** instead of SwiftUI + TCA
- **sherpa-onnx** instead of FluidAudio/Core ML
- **cpal** instead of AVAudioEngine
- **evdev/X11/Wayland** instead of Sauce
- **xdotool/wtype** instead of NSPasteboard

---

## Development Plan

### Phase 1: Make the CLI Genuinely Useful

#### 1.1 Streaming mic with VAD (auto-detect speech)
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/bin/canario-cli.rs`, `src/audio/mod.rs`

Use `sherpa_onnx::VoiceActivityDetector` to:
- Continuously capture mic audio
- Auto-detect when speech starts and ends
- Feed each speech segment to the recognizer
- Print results in real-time (no Ctrl+C needed)

sherpa-onnx already has a VAD example at `rust-api-examples/examples/parakeet_tdt_simulate_streaming_microphone.rs` — we should follow that pattern.

**Acceptance criteria:**
- `canario-cli --mic` records continuously
- Automatically detects speech segments
- Transcribes each segment and prints result
- Shows interim results while speaking

#### 1.2 Auto-paste after transcription
**Status:** 🔲 Not started (code exists in `src/ui/paste.rs`, untested)
**Difficulty:** Easy
**Files:** `src/bin/canario-cli.rs`, `src/ui/paste.rs`

Wire up `paste_text()` from `src/ui/paste.rs` to the CLI output. Add `--paste` flag.

**Acceptance criteria:**
- `canario-cli --mic --paste` auto-types the result into the focused app
- Works on both X11 (xdotool) and Wayland (wtype)
- Falls back to clipboard if direct typing fails

#### 1.3 Press-and-hold via CLI (optional)
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/bin/canario-cli.rs`, `src/hotkey/mod.rs`

Allow starting/stopping recording with a key press, not just Ctrl+C. Options:
- Use a simple key listener (evdev or stdin)
- Or use a named pipe/FIFO for external triggering

**Acceptance criteria:**
- Press key → start recording
- Release key → stop, transcribe, paste
- Or: tap key to toggle recording on/off

---

### Phase 2: GTK4 System Tray App

#### 2.1 GTK4 + Adwaita app skeleton
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/main.rs`, `src/ui/`

Create the base application:
- GTK4 Application with Adwaita
- System tray icon (Ayatana AppIndicator)
- Main window with settings
- Background service that listens for hotkey

**Acceptance criteria:**
- `canario` launches a GTK4 app with system tray icon
- Can open settings from tray menu
- Can quit from tray menu
- App runs in background

#### 2.2 Settings UI
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/ui/settings.rs`, `src/config/mod.rs`

Settings panel with:
- Model selection (v2 English / v3 Multilingual / Custom)
- Model download with progress bar
- Language selection
- Audio behavior during recording (do nothing / mute)
- Auto-paste toggle
- Transcription history toggle

**Acceptance criteria:**
- All settings persist to `~/.config/canario/config.json`
- Model download shows progress
- Can switch between model variants

#### 2.3 Recording indicator overlay
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/ui/indicator.rs`

A small floating overlay (like Hex) that shows:
- Recording state (red dot / waveform)
- Audio level meter
- "Transcribing..." state

**Acceptance criteria:**
- Appears when recording starts
- Shows audio level in real-time
- Changes state to "Transcribing..."
- Disappears after paste

#### 2.4 Model manager UI
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/ui/model_manager.rs`

Download/delete models from the settings UI:
- Show available models with size
- Download with progress bar
- Delete cached models
- Show currently active model

**Acceptance criteria:**
- Can download models from settings
- Progress bar shows download status
- Can switch between downloaded models

---

### Phase 3: Global Hotkey

#### 3.1 X11 global hotkey (XGrabKey)
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/hotkey/x11.rs`

Use X11 XGrabKey to detect key press and release globally:
- Detect when the configured hotkey is pressed → start recording
- Detect when released → stop recording, transcribe, paste
- Handle modifier-only hotkeys (like just Super or just Alt)

**Acceptance criteria:**
- Hotkey works in any X11 app
- Press-and-hold works (press → record, release → stop)
- Double-tap to lock works
- Doesn't interfere with normal app usage

#### 3.2 Wayland global hotkey
**Status:** 🔲 Not started
**Difficulty:** Hard
**Files:** `src/hotkey/wayland.rs`

Wayland doesn't allow global key grabbing. Options (in order of preference):
1. **XDG Desktop Portal** (GlobalShortcuts) — standardized but not widely implemented yet
2. **evdev** — read from `/dev/input/event*` (requires udev rules or root)
3. **GNOME Extension** — write a small extension that sends D-Bus signals
4. **Fallback** — system shortcut that runs `canario-cli --toggle`

**Acceptance criteria:**
- At least one method works on Wayland (GNOME + KDE)
- Document setup instructions for users
- Fallback to system shortcut method

#### 3.3 HotKeyProcessor logic
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** `src/hotkey/processor.rs`

Port Hex's `HotKeyProcessor` logic to Rust:
- Press-and-hold detection with minimum key time
- Double-tap detection with configurable timeout
- Modifier-only hotkey handling (0.3s threshold)
- Cancel on Escape or mouse click

**Acceptance criteria:**
- Behaves identically to Hex's hotkey system
- Configurable minimum key time
- Configurable double-tap behavior

---

### Phase 4: Polish

#### 4.1 Word remapping
**Status:** 🔲 Not started
**Difficulty:** Easy
**Files:** `src/config/mod.rs`, `src/inference/postprocess.rs`

Post-process transcription text:
- User-defined word remappings (e.g., "I llama" → "I'll ama")
- Word removals (remove filler words)
- Similar to Hex's WordRemapping/WordRemoval

#### 4.2 Transcription history
**Status:** 🔲 Not started
**Difficulty:** Easy
**Files:** `src/history/`

Store past transcriptions:
- JSON file or SQLite
- Timestamp, text, duration, source app
- Browseable history UI

#### 4.3 Sound effects
**Status:** 🔲 Not started
**Difficulty:** Easy
**Files:** `src/audio/effects.rs`

Play sounds on:
- Recording start (beep)
- Recording stop (double beep)
- Transcription pasted (confirmation sound)

Use `libpulse-simple` or `rodio` crate.

#### 4.4 Autostart + .desktop file
**Status:** 🔲 Not started
**Difficulty:** Easy
**Files:** `assets/canario.desktop`, `src/config/autostart.rs`

- Install .desktop file to `~/.local/share/applications/`
- Option to autostart on login (symlink to `~/.config/autostart/`)
- Icon in system menu

#### 4.5 Packaging
**Status:** 🔲 Not started
**Difficulty:** Medium
**Files:** ` packaging/`

- AppImage (most universal)
- .deb package (Debian/Ubuntu)
- Flatpak (sandboxed)
- Cargo install

---

## Key Technical Decisions

### Why sherpa-onnx instead of rolling our own?
- Already implements the full Parakeet TDT pipeline (preprocessor + encoder + decoder + joiner)
- Official Rust bindings maintained by the sherpa-onnx team
- Handles mel spectrogram, TDT decoding, vocabulary lookup internally
- Supports INT8 quantized models for fast CPU inference
- ~36x realtime on desktop CPU per benchmarks
- Would be weeks of work to reimplement from scratch

### Why INT8 quantized?
- 640MB vs 2.4GB (full precision)
- Minimal accuracy loss
- ~2-3x faster inference on CPU
- Fits comfortably on any modern laptop

### Wayland hotkey challenge
This is the hardest unsolved problem. On macOS, Hex uses accessibility APIs. On X11, we have XGrabKey. On Wayland, there's no standard global hotkey API yet.

The most practical approach for v1:
1. Detect if running on X11 → use XGrabKey (full experience)
2. Detect if running on Wayland → show instructions to set a system keyboard shortcut that runs `canario --toggle-recording`
3. Future: implement XDG GlobalShortcuts portal when it's widely available

---

## File Structure Plan (Target)

```
src/
├── main.rs                     # GTK4 app entry point
├── bin/
│   └── canario-cli.rs          # CLI tool
├── audio/
│   ├── mod.rs                  # Audio capture, ring buffer
│   └── effects.rs              # Sound effects
├── config/
│   ├── mod.rs                  # AppConfig, persistence
│   └── autostart.rs            # .desktop file management
├── history/
│   └── mod.rs                  # Transcription history
├── hotkey/
│   ├── mod.rs                  # Hotkey abstraction
│   ├── processor.rs            # Press-hold / double-tap logic
│   ├── x11.rs                  # X11 XGrabKey implementation
│   └── wayland.rs              # Wayland implementation
├── inference/
│   ├── mod.rs                  # TranscriptionEngine
│   └── postprocess.rs          # Word remapping
└── ui/
    ├── mod.rs                  # AppState, main loop
    ├── app.rs                  # GTK4 Application
    ├── tray.rs                 # System tray icon
    ├── settings.rs             # Settings window
    ├── indicator.rs            # Recording overlay
    ├── model_manager.rs        # Model download UI
    └── paste.rs                # xdotool/wtype paste
```

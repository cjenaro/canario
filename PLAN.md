# Canario Development Plan

> Native Linux voice-to-text using Parakeet TDT, inspired by [Hex](https://github.com/kitlangton/Hex) for macOS.

## What Works (v0.1.0)

### CLI (`canario-cli`)
- ✅ Transcribes WAV files and live microphone audio
- ✅ Downloads INT8 quantized Parakeet TDT v3 model from HuggingFace (~640MB)
- ✅ `sherpa-onnx` Rust bindings for ONNX inference + TDT decoding
- ✅ Audio capture via `cpal`, resampling, WAV reading/writing
- ✅ VAD-based streaming mic with Silero VAD
- ✅ Toggle mode (press Enter to start/stop)
- ✅ Auto-paste transcription into focused app

### GUI (`canario`)
- ✅ GTK4 + Adwaita system tray app
- ✅ Start/Stop recording from tray menu
- ✅ Settings window: model selection, download, auto-paste toggle
- ✅ Recording indicator overlay with audio level
- ✅ Transcription → auto-paste into focused app
- ✅ Clipboard fallback on Wayland without auto-type tools

## Quick Start

### CLI

```bash
# Download model (ASR + VAD)
./target/release/canario-cli --download

# Transcribe a WAV file
./target/release/canario-cli --wav recording.wav

# Stream from mic with VAD auto-detect (speak naturally)
./target/release/canario-cli --mic

# Stream from mic + auto-paste into focused app
./target/release/canario-cli --mic --paste

# Toggle mode: press Enter to start/stop recording
./target/release/canario-cli --mic --toggle
```

### GUI

```bash
# Build (requires GTK4 dev headers)
sudo apt install libgtk-4-dev libadwaita-1-dev   # one-time
cargo build --release --bin canario

# Run
./target/release/canario
```

Click the microphone tray icon → **Start Recording** → speak → **Stop Recording**.
The transcription is automatically pasted into your focused app.

## Auto-Paste Behavior

Canario always copies the transcription to your clipboard. Auto-typing
(into the focused app) is best-effort depending on available tools:

| Environment | Auto-type tool | Install |
|-------------|---------------|----------|
| **X11** | `xdotool` | `sudo apt install xdotool` |
| **Wayland (KDE, Hyprland, Sway)** | `wtype` | `sudo apt install wtype` |
| **Wayland (GNOME)** | `ydotool` + `ydotoold` | see below |

### GNOME Wayland (most common)

GNOME's Mutter compositor doesn't support the virtual keyboard protocol that
`wtype` needs. Use `ydotool` instead:

```bash
# Install CLI + daemon
sudo apt install ydotool ydotoold

# Create systemd service (daemon package doesn't include one)
echo '[Unit]
Description=ydotool daemon
After=local-fs.target

[Service]
Type=simple
ExecStart=/usr/bin/ydotoold
ExecStartPost=/bin/chmod 666 /tmp/.ydotool_socket
Restart=on-failure

[Install]
WantedBy=multi-user.target' | sudo tee /etc/systemd/system/ydotoold.service

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable --now ydotoold
```

The `ExecStartPost` line makes the daemon socket readable by all users.
Without it, `ydotool` can't connect and falls back to `/dev/uinput` (needs root).

Test: `ydotool type "hello world"` — should type into your focused app.

**If no auto-type tool works**, the transcription is still copied to your
clipboard — just press **Ctrl+V** to paste.

## Build Requirements

### CLI only (no GUI deps)
```bash
cargo build --release --no-default-features --features static --bin canario-cli
```

### GUI (requires system packages)
```bash
sudo apt install libgtk-4-dev libadwaita-1-dev
cargo build --release --bin canario
```

The `-dev` packages are **only needed for compiling**. End users running the
binary only need the runtime libraries (`libgtk-4-1`, `libadwaita-1-0`) which
are pre-installed on virtually every GNOME desktop.

## Architecture

```
src/
├── main.rs                 # GTK4 + Adwaita app entry point
├── bin/canario-cli.rs      # ✅ Working CLI prototype
├── audio/mod.rs            # Audio capture (cpal + ring buffer)
├── config/mod.rs           # AppConfig (JSON persistence)
├── hotkey/mod.rs           # Global hotkey (placeholder)
├── inference/mod.rs        # TranscriptionEngine wrapper
└── ui/
    ├── mod.rs              # AppState, AppMessage, AppStatus
    ├── app.rs              # GTK4 Application + main loop
    ├── tray.rs             # System tray (ksni D-Bus)
    ├── settings.rs         # Settings window
    ├── indicator.rs        # Recording overlay
    ├── model_manager.rs    # Model download/delete UI
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

### VAD Model

| Model | Source | Size |
|-------|--------|------|
| Silero VAD | `snakers4/silero-vad` | ~2.3 MB |

Stored at: `~/.local/share/canario/models/silero_vad.onnx`

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
**Status:** ✅ Done
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
**Status:** ✅ Done
**Difficulty:** Easy
**Files:** `src/bin/canario-cli.rs`, `src/ui/paste.rs`

Wire up `paste_text()` from `src/ui/paste.rs` to the CLI output. Add `--paste` flag.

**Acceptance criteria:**
- `canario-cli --mic --paste` auto-types the result into the focused app
- Works on both X11 (xdotool) and Wayland (wtype)
- Falls back to clipboard if direct typing fails

#### 1.3 Press-and-hold via CLI (optional)
**Status:** ✅ Done
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
**Status:** ✅ Done (code written, requires libgtk-4-dev + libadwaita-1-dev to compile)
**Difficulty:** Medium
**Files:** `src/main.rs`, `src/ui/app.rs`, `src/ui/tray.rs`

Create the base application:
- GTK4 Application with Adwaita (using `adw::Application`)
- System tray icon via `ksni` (D-Bus StatusNotifierItem, no GTK3 dependency)
- Background service that listens for tray actions via `mpsc` channel
- Communication between tray thread and GTK main loop via `glib::timeout_add_local` polling

**Note:** Building the GUI requires `sudo apt install libgtk-4-dev libadwaita-1-dev`

**Acceptance criteria:**
- ✅ `canario` launches a GTK4 app with system tray icon
- ✅ Can open settings from tray menu
- ✅ Can quit from tray menu
- ✅ App runs in background (uses `app.hold()` with leaked guard)
- ✅ `--no-default-features` builds CLI without GUI deps

#### 2.2 Settings UI
**Status:** ✅ Done (code written)
**Difficulty:** Medium
**Files:** `src/ui/settings.rs`, `src/ui/model_manager.rs`, `src/config/mod.rs`

Settings panel with:
- Model selection (v3 Multilingual / v2 English) via `adw::ComboRow`
- Model download with progress bar (background download via tokio)
- Model delete button
- Audio behavior during recording (do nothing / mute)
- Auto-paste toggle via `adw::SwitchRow`
- Double-tap to lock toggle
- Hotkey info row (placeholder for Phase 3)

**Acceptance criteria:**
- ✅ All settings persist to `~/.config/canario/config.json`
- ✅ Model download shows progress (pulsing bar)
- ✅ Can switch between model variants
- ✅ Settings window is singleton (re-opened if already exists)

#### 2.3 Recording indicator overlay
**Status:** ✅ Done (code written)
**Difficulty:** Medium
**Files:** `src/ui/indicator.rs`

A small floating overlay that shows:
- Recording state (🔴 dot + "Recording…" label)
- Audio level progress bar
- Styled with OSD CSS classes
- Undecorated popup window

**Note:** True layer-shell overlay positioning requires `gtk4-layer-shell` crate (can be added in Phase 4)

**Acceptance criteria:**
- ✅ Appears when recording starts
- ✅ Shows audio level in real-time
- ✅ Disappears when recording stops

**Acceptance criteria:**
- Appears when recording starts
- Shows audio level in real-time
- Changes state to "Transcribing..."
- Disappears after paste

#### 2.4 Model manager UI
**Status:** ✅ Done (code written)
**Difficulty:** Medium
**Files:** `src/ui/model_manager.rs`

Download/delete models from the settings UI:
- Show model status (downloaded / not downloaded / downloading)
- Download button triggers background download with progress
- Delete button removes model files
- Download uses separate thread + `glib::timeout_add_local` for UI updates

**Acceptance criteria:**
- ✅ Can download models from settings
- ✅ Progress bar pulses during download
- ✅ Can delete cached models
- ✅ Can switch between downloaded models

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

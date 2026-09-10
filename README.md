# Canario — Voice → Text

Press-and-hold a hotkey to transcribe your voice and paste the result wherever you're typing.

Inspired by [Hex](https://github.com/kitlangton/Hex) for macOS, powered by [NVIDIA Parakeet TDT](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) via [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

> **Disclaimer:** Canario is a cheap knockoff of [Hex](https://github.com/kitlangton/Hex), ported to Linux and cross-platform via Electron. All credit for the original idea and UX goes to the Hex team. This project exists because Hex is macOS-only and I wanted something that worked on my machine.

## Download

### Linux

> **Note:** The release pipeline is currently being rebuilt, so prebuilt
> installers are not available yet. For now, [build from source](#build-from-source)
> (it's quick — see below).

Once releases are published again, you'll be able to grab an installer from
the [latest release](https://github.com/cjenaro/canario/releases/latest):

| File | Notes |
|------|-------|
| `Canario-*.AppImage` | Self-contained, no dependencies |
| `Canario-*.deb` | Debian/Ubuntu package |

On first launch, you'll be prompted to download the ASR model (~640MB). Everything runs locally — nothing leaves your machine.

### Auto-update

The Electron app includes auto-update support (checks GitHub Releases every
4 hours and notifies when a new version is ready), but it only works with
published releases — which are not currently available while the release
pipeline is being fixed.

### macOS / Windows

Not yet available. `canario-core` depends on Linux-specific libraries (evdev, X11). Cross-platform support requires adding `#[cfg(target_os)]` guards — tracked in the PRD as future work.

### Build from source

<details>
<summary>Linux (GTK4 native)</summary>

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt install build-essential cmake clang libgtk-4-dev libadwaita-1-dev libappindicator3-dev

# Build and run
cargo build --release
cargo run --release --bin canario
```

</details>

<details>
<summary>Linux (Electron development build)</summary>

```bash
# From the repository root, build the debug sidecar used by dev mode
cargo build --bin canario-electron

# Install frontend dependencies (requires Node.js 22+)
cd canario-app
npm ci

# Build and launch the Electron app in dev mode
npm run dev
```

Dev mode loads `target/debug/canario-electron`, so a release-only Rust build
will not work here. After changing Rust code, rebuild the sidecar and restart
the app. Frontend edits reload automatically.

On first launch, download a model. Test recording with the **Record** button,
then test the global hotkey and auto-paste into a text editor. Run one Canario
frontend at a time so they do not compete for the hotkey or microphone.

To test the native GTK frontend instead, run `cargo run --bin canario` from
the repository root (requires the GTK development packages listed above).

</details>

### Build a local Linux AppImage

To run a packaged app without the development server, build and stage the
release sidecar, then package the frontend (from the repository root):

```bash
cargo build --release --bin canario-electron
mkdir -p canario-app/sidecar
cp -f target/release/canario-electron canario-app/sidecar/
cd canario-app
npm ci
npm run build
npx electron-builder --linux AppImage --publish never
chmod +x dist/Canario-*.AppImage
./dist/Canario-*.AppImage
```

The AppImage runs directly; no system installation is needed. If your system
does not have FUSE support, launch it with `--appimage-extract-and-run`.

#### Chromium sandbox on restricted kernels

Ubuntu ≥ 24.04 sets `kernel.apparmor_restrict_unprivileged_userns=1`, which
blocks Chromium's namespace sandbox. An AppImage mounts as your own user, so
its bundled `chrome-sandbox` can never be setuid root and the app aborts at
startup with a SUID-sandbox `FATAL` error. Pick one of:

```bash
# 1. Launch the AppImage with the sandbox disabled
./dist/Canario-*.AppImage --no-sandbox

# 2. Prefer a real install: the .deb postinst chmods chrome-sandbox 4755
sudo apt install ./dist/canario-app_*_amd64.deb

# 3. Or re-enable unprivileged user namespaces system-wide (keeps the sandbox)
echo kernel.apparmor_restrict_unprivileged_userns=0 | \
  sudo tee /etc/sysctl.d/99-canario-userns.conf && sudo sysctl --system
```

Build both installers in one invocation — electron-builder removes artifacts
of targets not named on the command line:

```bash
npx electron-builder --linux AppImage deb --publish never
```


### Model-backed smoke test

The regular Rust tests do not require a downloaded model. To also exercise the
actual ONNX recognizer with a synthetic WAV, point this opt-in test at an
existing model directory (it does not download or modify model files):

```bash
CANARIO_TEST_MODEL_DIR="$HOME/.local/share/canario/models/sherpa-parakeet-tdt-v3" \
  cargo test -p canario-core --test wav_integration \
  transcribes_synthetic_wav_with_cached_model -- --ignored
```

## How it works

### Hotkeys

1. **Press-and-hold** the hotkey (default: Super+Space) → record → release → transcribe → paste
2. **Double-tap** to lock recording → tap again to stop and transcribe

### Auto-paste

After transcription, the text is pasted into whatever app has focus.

| Environment | Method |
|-------------|--------|
| **Linux X11** | `xdotool` |
| **Linux Wayland** | `wtype` or `ydotool` |
| **macOS** | Clipboard + Cmd+V (requires Accessibility permission) |
| **Windows** | Clipboard + Ctrl+V |

If auto-paste isn't available, the transcription is still copied to your clipboard — just press Ctrl/Cmd+V.

### Models

- **Parakeet TDT v3** — multilingual (EN, ES, FR, DE, etc.) · ~640MB INT8
- **Parakeet TDT v2** — English only · ~640MB INT8

Both run entirely on-device via ONNX Runtime. No internet connection required after download.

## Architecture

```
┌─────────────────────────────────────────┐
│  Frontend (GTK4 or Electron + SolidJS)  │
│  System tray, overlay, settings         │
├─────────────────────────────────────────┤
│  Canario Core (Rust)                    │
│  Hotkey → Record → Transcribe → Paste   │
├─────────────────────────────────────────┤
│  sherpa-onnx (Rust/C++ via FFI)         │
│  ┌───────────────────────────────────┐  │
│  │ ONNX Runtime                      │  │
│  │ • Encoder (conformer)             │  │
│  │ • Decoder + Joint (LSTM + TDT)    │  │
│  ├───────────────────────────────────┤  │
│  │ Mel spectrogram preprocessor      │  │
│  ├───────────────────────────────────┤  │
│  │ TDT greedy decoder               │  │
│  └───────────────────────────────────┘  │
├─────────────────────────────────────────┤
│  cpal (mic capture) → 16kHz mono       │
│  Ring buffer for instant start          │
├─────────────────────────────────────────┤
│  xdotool / wtype / robotjs (paste)     │
└─────────────────────────────────────────┘
```

## License

MIT

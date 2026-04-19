# Canario — Voice → Text

Press-and-hold a hotkey to transcribe your voice and paste the result wherever you're typing.

Inspired by [Hex](https://github.com/kitlangton/Hex) for macOS, powered by [NVIDIA Parakeet TDT](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) via [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

> **Disclaimer:** Canario is a cheap knockoff of [Hex](https://github.com/kitlangton/Hex), ported to Linux and cross-platform via Electron. All credit for the original idea and UX goes to the Hex team. This project exists because Hex is macOS-only and I wanted something that worked on my machine.

## Download

### Linux · macOS · Windows

Go to the [latest release](https://github.com/cjenaro/canario/releases/latest) and download the installer for your platform:

| Platform | File |
|----------|------|
| **Linux** | `Canario-0.1.2.AppImage` or `Canario-0.1.2.deb` |
| **macOS (Apple Silicon)** | `Canario-0.1.2-arm64.dmg` |
| **macOS (Intel)** | `Canario-0.1.2-x64.dmg` |
| **Windows** | `Canario-Setup-0.1.2.exe` |

On first launch, you'll be prompted to download the ASR model (~640MB). Everything runs locally — nothing leaves your machine.

**macOS:** Right-click the app → Open on first launch (unsigned build).

**Windows:** Click "More info" → "Run anyway" on the SmartScreen prompt.

### Auto-update

The app checks for updates automatically every 4 hours and notifies you when a new version is ready to install.

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
<summary>Cross-platform (Electron)</summary>

```bash
# Build Rust sidecar
cargo build --release --bin canario-electron

# Build Electron app (requires Node.js 22+)
cd canario-app
npm ci
npm run build

# Run in dev mode
npm run dev
```

</details>

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

# Canario — Voice → Text for Linux

Press-and-hold a hotkey to transcribe your voice and paste the result wherever you're typing.

Inspired by [Hex](https://github.com/kitlangton/Hex) for macOS, powered by [NVIDIA Parakeet TDT](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) via [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx).

## Architecture

```
┌────────────────────────────────────────┐
│  GTK4 + Adwaita UI (Relm4)            │
│  System tray, settings, indicator      │
├────────────────────────────────────────┤
│  Canario Core                          │
│  Hotkey → Record → Transcribe → Paste  │
├────────────────────────────────────────┤
│  sherpa-onnx (Rust/C++ via FFI)        │
│  ┌──────────────────────────────────┐  │
│  │ ONNX Runtime                     │  │
│  │ • Encoder (conformer)            │  │
│  │ • Decoder + Joint (LSTM + TDT)   │  │
│  ├──────────────────────────────────┤  │
│  │ Mel spectrogram preprocessor     │  │
│  │ (built into sherpa-onnx)         │  │
│  ├──────────────────────────────────┤  │
│  │ TDT greedy decoder              │  │
│  │ (built into sherpa-onnx)         │  │
│  └──────────────────────────────────┘  │
├────────────────────────────────────────┤
│  cpal (mic capture) → 16kHz mono      │
│  Ring buffer for instant start         │
├────────────────────────────────────────┤
│  xdotool / wtype (text paste)          │
└────────────────────────────────────────┘
```

## Models

Uses ONNX exports from HuggingFace:
- **Parakeet TDT v3** (multilingual, 640MB INT8): `istupakov/parakeet-tdt-0.6b-v3-onnx`
- **Parakeet TDT v2** (English, 640MB INT8): `istupakov/parakeet-tdt-0.6b-v2-onnx`

## Building

```bash
# Install dependencies (Ubuntu/Debian)
sudo apt install build-essential cmake clang libgtk-4-dev libadwaita-1-dev libappindicator3-dev

# Build
cargo build --release
```

## Running

```bash
# First run will prompt to download model
cargo run --release

# Or specify model path
canario --encoder ./models/encoder-model.int8.onnx \
        --decoder ./models/decoder_joint-model.int8.onnx \
        --tokens ./models/vocab.txt
```

## Hotkeys

1. **Press-and-hold** the hotkey (default: Super+Space) → record → release → transcribe → paste
2. **Double-tap** to lock recording → tap again to stop and transcribe

## License

MIT

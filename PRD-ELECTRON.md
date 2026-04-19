# Canario Electron — Product Requirements Document

> Cross-platform voice-to-text desktop app built on `canario-core`.
> Performance and UI elegance are top priority.

**Status:** Draft — v0.2  
**Date:** 2026-04-18  
**Authors:** Jenaro  
**Framework:** SolidJS (see §9 for rationale)  
**State management:** State machine (see §3.4)  

---

## 1. Vision

Canario Electron is a **desktop app that makes voice-to-text feel instant and invisible**. You press a hotkey, speak, release — and your words appear wherever your cursor is. The app itself should get out of the way: minimal chrome, fast launch, no visual noise.

The Electron frontend is a peer to the existing GTK and CLI frontends. It shares the same `canario-core` backend, the same config files, the same model storage. Users choose whichever frontend fits their platform or preference.

**Why Electron when we already have GTK?**

- **Cross-platform** — GTK is Linux-only. Electron gives us macOS and Windows for free.
- **UI velocity** — HTML/CSS/JS iterates faster than GTK/Relm4. Better animations, easier theming.
- **Consistent experience** — same look on every OS, no native toolkit differences.
- **Ecosystem** — tray, auto-update, notifications, rich settings UI — all batteries-included.

---

## 2. Guiding Principles

| Principle | Implication |
|-----------|-------------|
| **Zero-touch after setup** | Once configured, the app is invisible. System tray only. No windows unless the user opens them. |
| **Instant feedback** | Recording indicator appears in <50ms. Audio level updates at 20fps. No perceptible delay between "stop speaking" and "see text". |
| **Small footprint** | Sidecar Rust binary does all heavy lifting. Electron process stays light — no audio processing, no ML inference in JS. Target <80MB RAM total (Electron + sidecar idle). |
| **Separation of concerns** | `canario-core` is the brain. The sidecar is the spinal cord. Electron is the face. No overlap, no duplication. |
| **Config compatibility** | Same `~/.config/canario/config.json` and `~/.local/share/canario/` data. Switch frontends without migrating. |

---

## 3. Architecture

### 3.1 Process Topology

```
┌──────────────────────────────────────────────────────────┐
│  Electron Main Process                                    │
│  ┌────────────┐  ┌────────────┐  ┌─────────────────────┐ │
│  │ System Tray │  │ Global     │  │ Sidecar Manager     │ │
│  │ (native)    │  │ Shortcut   │  │ spawn + IPC pipe    │ │
│  └────────────┘  └────────────┘  └────────┬────────────┘ │
│                                            │ stdout/stdin │
│  ┌─────────────────────────────────────────▼────────────┐ │
│  │  IPC Bridge (preload.ts → Solid primitives)           │ │
│  └──────────────────────────────────────────────────────┘ │
│                            IPC                              │
│  ┌──────────────────────────────────────────────────────┐ │
│  │  Renderer Process (SolidJS + Vite)                     │ │
│  │  ┌──────────┐ ┌──────────┐ ┌───────┐ ┌────────────┐ │ │
│  │  │ Recording│ │ Settings │ │History│ │ Onboarding │ │ │
│  │  │ Overlay  │ │          │ │       │ │            │ │ │
│  │  └──────────┘ └──────────┘ └───────┘ └────────────┘ │ │
│  └──────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
                             │
                    stdin/stdout JSON
                             │
┌──────────────────────────────────────────────────────────┐
│  canario-electron (Rust sidecar binary)                    │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │ stdin → CMD  │  │ Event loop   │  │ stdout → EVENT │  │
│  │ parser       │  │ (Canario::   │  │ serializer     │  │
│  │              │  │  new())      │  │                │  │
│  └──────────────┘  └──────┬───────┘  └────────────────┘  │
│                           │                                │
│  ┌────────────────────────▼───────────────────────────┐   │
│  │  canario-core                                       │   │
│  │  • Audio capture (cpal)                             │   │
│  │  • ASR inference (sherpa-onnx / Parakeet TDT)       │   │
│  │  • Hotkey listener (evdev / X11)                    │   │
│  │  • Auto-paste (xdotool / wtype / ydotool)           │   │
│  │  • Model management                                 │   │
│  │  • History + config persistence                     │   │
│  └────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────┘
```

### 3.2 Why Sidecar, Not Native Addon

| Factor | Sidecar (chosen) | Native addon (napi-rs/neon) |
|--------|-------------------|-----------------------------|
| Build | `cargo build` → static binary | C++ toolchain + node-gyp per Electron version |
| Portability | one binary per OS+arch | rebuild per Electron major version |
| Hotkey access | ✅ native process, full evdev/X11 | ❌ Electron sandbox blocks low-level input |
| Audio capture | ✅ cpal in Rust, zero-copy | ⚠️ possible but painful through Node |
| Auto-paste | ✅ native process | ⚠️ needs child_process anyway |
| IPC latency | ~0.5ms (stdin/stdout JSON on localhost) | 0ms |
| Crash isolation | sidecar crash ≠ Electron crash | segfault takes down everything |
| Debugging | run sidecar standalone in terminal | tied to Node lifecycle |

**Decision:** Sidecar for v1. If audio-level streaming latency becomes measurable (unlikely at 20Hz updates), native addon can be revisited as a v2 optimization.

### 3.3 IPC Protocol

**Transport:** newline-delimited JSON over stdin/stdout. Binary builds on the existing `Event` enum and `Canario` methods.

#### Commands → Sidecar (stdin)

```jsonc
// Recording
{"id":"1","cmd":"start_recording"}
{"id":"2","cmd":"stop_recording"}
{"id":"3","cmd":"toggle_recording"}

// Model
{"id":"4","cmd":"download_model"}
{"id":"5","cmd":"delete_model"}
{"id":"6","cmd":"is_model_downloaded"}

// Config
{"id":"7","cmd":"get_config"}
{"id":"8","cmd":"update_config","config":{"auto_paste":false}}

// History
{"id":"9","cmd":"get_history","limit":50}
{"id":"10","cmd":"search_history","query":"hello"}
{"id":"11","cmd":"delete_history","id":"uuid-here"}
{"id":"12","cmd":"clear_history"}

// Hotkey
{"id":"13","cmd":"start_hotkey"}
{"id":"14","cmd":"stop_hotkey"}
{"id":"15","cmd":"restart_hotkey"}

// Lifecycle
{"id":"16","cmd":"ping"}
{"id":"17","cmd":"shutdown"}
```

#### Events → Electron (stdout)

```jsonc
// Async events (no id — pushed by sidecar at any time)
{"event":"RecordingStarted"}
{"event":"RecordingStopped"}
{"event":"TranscriptionReady","text":"hello world","duration_secs":3.2}
{"event":"AudioLevel","level":0.65}
{"event":"Error","message":"No microphone found"}
{"event":"ModelDownloadProgress","progress":0.42}
{"event":"ModelDownloadComplete"}
{"event":"ModelDownloadFailed","error":"Network timeout"}
{"event":"HotkeyTriggered"}

// Command responses (include the request id)
{"id":"1","ok":true}                           // success
{"id":"1","ok":false,"error":"Already recording"} // failure
{"id":"7","ok":true,"data":{...config...}}     // response with payload
{"id":"6","ok":true,"data":true}               // is_model_downloaded
```

#### Design Decisions

- **`id` field** — lets Electron match async responses to commands. Sidecar echoes it back.
- **Events have no `id`** — they're unsolicited, pushed by the core whenever they happen.
- **Newline-delimited** — simple, no framing issues. `readline()` on both sides.
- **No binary framing** — JSON is fast enough at 20Hz event rate. If we ever need to stream raw audio, we'd add a separate binary channel.

### 3.4 Global State Machine

The Electron renderer uses a **custom state machine** for all global app state. This is the single source of truth that coordinates every component — tray, overlay, settings, history, onboarding.

#### Why a state machine?

The app has a small, well-defined set of states with strict transitions:

- You can't transcribe without recording first
- You can't record if the model isn't downloaded
- You can't download a model if one is already downloading
- The onboarding wizard must complete before the app goes to idle

A state machine makes **illegal states unrepresentable**. If you try to send `STOP_RECORDING` while `idle`, nothing happens — the transition doesn't exist. No `if (state === 'recording')` guards scattered across components. The machine is the guard.

This also makes the app **impossible to break**: no race condition between hotkey press and model download, no zombie recording state after an error, no overlay stuck open. The machine enforces consistency.

#### States

```
                    ┌─────────────┐
          ┌────────▶│  Onboarding │──── (wizard complete) ───┐
          │         └─────────────┘                          │
          │                                                  ▼
    (first launch)                                   ┌──────────┐
          │                                          │  Idle    │◀──────────────────┐
          │                                          └────┬─────┘                   │
          │                                               │                         │
          │                                    (hotkey / tray "start")            │
          │                                               │                         │
          │                                               ▼                         │
          │                                        ┌──────────┐                   │
          │                                        │Recording │──── (error) ────▶│
          │                                        └────┬─────┘                   │
          │                                             │                         │
          │                                  (hotkey / tray "stop")              │
          │                                             │                         │
          │                                             ▼                         │
          │                                        ┌──────────────┐              │
          │                                        │Transcribing  │              │
          │                                        └────┬─────────┘              │
          │                                             │                         │
          │                              (TranscriptionReady / RecordingStopped)  │
          │                                             │                         │
          │                                             └─────────────────────────┘
          │                                                                       │
          │                                        ┌──────────────┐              │
          │                                        │  Downloading  │─────────────┘
          │                                        └──────┬───────┘   (complete/
          │                                               │            failed)
          │                                    (user triggers download          │
          │                                     from settings)                  │
          │                                               │                   │
          │                              ◀── (can enter from Idle only) ───────┘
          │
          └── (model exists? skip to Idle)
```

```typescript
type AppState =
  | { status: "onboarding"; step: number }
  | { status: "idle"; hasModel: boolean }
  | { status: "recording"; startedAt: number }
  | { status: "transcribing"; startedAt: number }
  | { status: "downloading"; progress: number };
```

#### Transitions

The transition map defines which events are valid in each state. Anything not in this map is silently ignored.

```typescript
const transitions = {
  onboarding: {
    WIZARD_COMPLETE: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  idle: {
    START_RECORDING: (ctx) => {
      if (!ctx.hasModel) return undefined; // reject — no model
      return { status: "recording", startedAt: Date.now() };
    },
    START_DOWNLOAD:  () => ({ status: "downloading", progress: 0 }),
  },
  recording: {
    STOP_RECORDING: (ctx) => ({ status: "transcribing", startedAt: Date.now() }),
    ERROR:          () => ({ status: "idle", hasModel: true }),
  },
  transcribing: {
    TRANSCRIPTION_READY: (ctx) => ({ status: "idle", hasModel: true }),
    RECORDING_STOPPED:   (ctx) => ({ status: "idle", hasModel: true }), // empty / error
    ERROR:               () => ({ status: "idle", hasModel: true }),
  },
  downloading: {
    DOWNLOAD_PROGRESS: (ctx, event) => ({ ...ctx, progress: event.progress }),
    DOWNLOAD_COMPLETE: () => ({ status: "idle", hasModel: true }),
    DOWNLOAD_FAILED:   () => ({ status: "idle", hasModel: false }),
  },
};
```

#### Context (extended state)

The machine carries a context object alongside the state. This is data that persists across transitions but doesn't define the state itself:

```typescript
type AppContext = {
  modelReady: boolean;     // is the ASR model downloaded?
  lastTranscription: string | null;
  lastError: string | null;
  config: AppConfig;       // snapshot of sidecar config
};
```

Context is updated by transitions and read by components. It's not a separate store — it lives inside the machine.

#### Implementation: custom, ~80 lines, no library

```typescript
// src/state/machine.ts
import { createSignal, createEffect, onCleanup } from "solid-js";
import type { AppState, AppEvent, AppContext } from "./types";
import { transitions } from "./types";

export function createAppMachine() {
  const [state, setState] = createSignal<AppState>(
    { status: "idle", hasModel: false }
  );
  const [context, setContext] = createSignal<AppContext>(defaultContext);

  function send(event: AppEvent) {
    const current = state();
    const status = current.status;
    const ctx = context();

    // Is this event valid in the current state?
    const handler = transitions[status]?.[event.type];
    if (!handler) return; // ignore invalid transitions

    const next = handler(ctx, event);
    if (!next) return; // guard rejected

    setState(next);
    // Context can also be updated by the handler
    if (event.contextUpdate) {
      setContext({ ...ctx, ...event.contextUpdate });
    }
  }

  return { state, context, send };
}
```

This is the entire machine. No external library. Solid signals make it reactive — any component that reads `state()` or `context()` automatically updates when the machine transitions.

#### How components use it

```tsx
// RecordingOverlay.tsx
function RecordingOverlay() {
  const { state } = useAppState(); // Solid context

  return (
    <Show when={state().status === "recording" || state().status === "transcribing"}>
      <div class="overlay">
        <Show when={state().status === "recording"} fallback={<p>Transcribing…</p>}>
          <RecordingDot />
          <AudioLevel />
        </Show>
      </div>
    </Show>
  );
}
```

```tsx
// Tray menu (in Electron main process, but reads same state via IPC)
// Settings page
function SettingsPage() {
  const { state, send } = useAppState();

  return (
    <Show when={state().status === "downloading"} fallback={
      <Button onClick={() => send({ type: "START_DOWNLOAD" })}>Download Model</Button>
    }>
      <Progress value={state().progress} />
    </Show>
  );
}
```

No component ever checks `if (isRecording && !isDownloading && modelReady)`. The machine already guarantees it. Components just match on `state().status` and render.

#### Why not XState / Robot / another library?

- Our state graph has **5 states** and **~10 transitions**. XState is designed for complex machines with hundreds of states, parallel regions, hierarchical composition. It would add ~15KB for a problem we solve in ~80 lines.
- `robot` is lighter but still an unnecessary dependency for this graph size.
- A custom machine built on Solid signals is: zero dependencies, fully typed, reactive by default, auditable in a single file, and debuggable with a `console.log` in `send()`.
- If the state graph grows significantly in future (unlikely — this is a voice-to-text app, not a workflow engine), we can migrate to XState then.

---

## 4. File Structure

```
canario/
├── canario-core/                  # ✅ untouched — shared backend
├── canario-gtk/                   # ✅ untouched — GTK4 frontend
├── canario-cli/                   # ✅ untouched — CLI frontend
├── canario-electron/              # 🆕 Rust sidecar binary
│   ├── Cargo.toml
│   └── src/
│       └── main.rs                # JSON stdin/stdout bridge over canario-core
├── canario-app/                   # 🆕 Electron application
│   ├── package.json
│   ├── tsconfig.json
│   ├── vite.config.ts             # Vite for renderer bundling
│   ├── electron/
│   │   ├── main.ts                # Electron main process entry
│   │   ├── preload.ts             # contextBridge IPC API
│   │   ├── sidecar.ts             # spawn + manage Rust sidecar
│   │   ├── tray.ts                # system tray icon + menu
│   │   ├── shortcuts.ts           # global keyboard shortcuts (macOS/Windows)
│   │   └── updater.ts             # auto-update logic
│   ├── src/                       # Renderer (SolidJS)
│   │   ├── index.tsx              # Solid entry — render(<App>)
│   │   ├── App.tsx                # root component, <Show> on app state
│   │   ├── index.css              # global styles + Tailwind
│   │   ├── state/
│   │   │   ├── machine.ts         # global state machine definition
│   │   │   ├── context.ts         # Solid context provider for the machine
│   │   │   └── types.ts           # AppState, AppEvent, transition map
│   │   ├── primitives/            # Solid reactive primitives (not React hooks)
│   │   │   ├── createCanario.ts   # sidecar IPC bridge
│   │   │   ├── createRecording.ts # recording-level signals
│   │   │   ├── createConfig.ts    # config read/write
│   │   │   └── createHistory.ts   # history queries
│   │   ├── components/
│   │   │   ├── ui/                # solid-ui base components (Kobalte + Tailwind)
│   │   │   ├── RecordingOverlay.tsx
│   │   │   ├── AudioLevel.tsx     # animated level meter
│   │   │   ├── Waveform.tsx       # real-time audio visualization
│   │   │   └── HotkeyCapture.tsx  # keyboard capture widget
│   │   ├── pages/
│   │   │   ├── Onboarding.tsx     # first-launch setup wizard
│   │   │   ├── Settings.tsx       # main settings page
│   │   │   ├── History.tsx        # transcription history browser
│   │   │   └── Popup.tsx          # quick-action popup (triggered by hotkey)
│   │   ├── lib/
│   │   │   ├── ipc.ts             # typed IPC channels
│   │   │   ├── sidecar-protocol.ts # command/event type definitions
│   │   │   └── utils.ts
│   │   └── styles/
│   │       ├── animations.css     # recording pulse, slide-in, fade
│   │       └── themes.css         # light/dark theme variables
│   ├── resources/
│   │   ├── icon.png               # app icon (1024x1024)
│   │   ├── icon.svg               # tray icon SVG
│   │   └── sounds/                # (optional) custom sound files
│   └── electron-builder.yml       # packaging config
├── Cargo.toml                     # add canario-electron to workspace members
├── PLAN.md
└── PRD-ELECTRON.md                # this file
```

### Workspace Change

```toml
# /Cargo.toml
[workspace]
members = ["canario-core", "canario-gtk", "canario-cli", "canario-electron"]
resolver = "2"
```

`canario-app/` stays outside the Rust workspace — it's a Node project with its own `package.json` and build toolchain.

---

## 5. Screens & UX Flows

### 5.1 First Launch — Onboarding Wizard

A 3-step setup that gets the user from "install" to "first transcription" in under 2 minutes.

```
┌─────────────────────────────────────────────────────────────┐
│                                                              │
│   🎙️  Welcome to Canario                                    │
│                                                              │
│   Voice-to-text, instant and invisible.                     │
│   Press a hotkey, speak, release. Done.                     │
│                                                              │
│   ┌───────────────────────────────────────────────────┐      │
│   │                                                    │      │
│   │   Step 1 of 3: Download Model                     │      │
│   │                                                    │      │
│   │   Canario uses Parakeet TDT — a state-of-the-art  │      │
│   │   speech recognition model that runs locally.      │      │
│   │                                                    │      │
│   │   Model: Parakeet TDT v3 (Multilingual)   ~640MB  │      │
│   │                                                    │      │
│   │   ┌──────────────────────────────────────────┐     │      │
│   │   │████████████████░░░░░░░░░░░░░░░░░░  42%   │     │      │
│   │   └──────────────────────────────────────────┘     │      │
│   │                                                    │      │
│   │   ┌──────────────────────────────────────────┐     │      │
│   │   │  🎤 Microphone Test                       │     │      │
│   │   │  Say something...                         │     │      │
│   │   │  ████████░░░░░░░░░░░░░░  (level meter)    │     │      │
│   │   └──────────────────────────────────────────┘     │      │
│   │                                                    │      │
│   │                                    [Next →]        │      │
│   └───────────────────────────────────────────────────┘      │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

**Step 1 — Download Model**
- Auto-selects v3 (multilingual) by default, dropdown to pick v2 (English)
- Download progress bar with speed + ETA
- Mic test widget: shows live audio level so user confirms mic works
- "Test microphone" button that records 2 seconds and plays back

**Step 2 — Set Hotkey**
- Interactive hotkey capture (same UX as GTK settings)
- Shows the chosen hotkey combo in real-time
- Explains press-and-hold vs double-tap modes
- On macOS/Windows: uses Electron's `globalShortcut` API
- On Linux: delegates to sidecar's evdev/X11 listener

**Step 3 — Ready**
- "Try it now!" prompt with a practice area
- User presses hotkey → records → sees transcription
- Auto-paste demo into a text field in the wizard itself
- Checkbox: "Start on login"
- Done → app minimizes to tray

### 5.2 System Tray (Idle State)

The app lives in the system tray. No windows open unless the user asks for them.

```
┌──────────────────┐
│  🎙️ Canario      │  ← tray icon (canary SVG, 22x22)
│                   │
│  ● Ready          │  ← status: Ready / Recording / Transcribing
│  ─────────────── │
│  ▶ Start Recording│  ← toggle: changes to ■ Stop when recording
│  ⚙ Settings       │
│  📋 History        │
│  ─────────────── │
│  Quit             │
└──────────────────┘
```

**Tray icon states:**
- **Default** — static canary icon
- **Recording** — icon pulses gently (red glow) OR icon changes to a red microphone
- **Transcribing** — brief spinner animation (~1-2s)

### 5.3 Recording Overlay

The signature visual moment. A small, elegant overlay that appears the instant recording starts.

```
┌────────────────────────────────┐
│  ● Recording                   │
│  ▓▓▓▓▓▓▓▓▓▓░░░░░░░░░  0.7    │
│  0:03                          │
└────────────────────────────────┘
```

**Spec:**
- **Size:** ~220×60px, positioned top-center of screen (configurable)
- **Appearance:** rounded corners, subtle backdrop-blur, semi-transparent dark background
- **Always on top** — `alwaysOnTop: true, focusable: false`
- **Content:**
  - Pulsing red dot (CSS animation, 1.5s cycle)
  - "Recording" label in system font
  - Audio level meter — smooth gradient bar, updated at 20fps
  - Elapsed timer (MM:SS)
- **Transition in:** fade + slide-down from top (150ms ease-out)
- **Transition out:** fade (100ms) — happens when transcription starts
- **State change to "Transcribing…":** red dot → spinning indicator, label changes, level bar freezes
- **Disappears** after transcription completes or on error

**Performance constraints:**
- Window must appear within **50ms** of `RecordingStarted` event
- Audio level updates must not drop frames — use `requestAnimationFrame` for the bar, decouple from IPC event rate
- Window creation: **pre-create** the overlay window on app start, hide it. Show/hide is near-instant. Don't create on demand.

### 5.4 Settings Window

A clean, single-column layout. Mirrors the GTK settings but with better visual hierarchy.

```
┌──────────────────────────────────────────────────────────────┐
│  ⚙ Canario Settings                                    ─ □ ✕ │
│──────────────────────────────────────────────────────────────│
│                                                               │
│  ┌─ Model ────────────────────────────────────────────────┐  │
│  │                                                        │  │
│  │  Model Variant        [Parakeet TDT v3 ▾]              │  │
│  │  Status               ✅ Downloaded (640MB)            │  │
│  │                                                        │  │
│  │  [Delete Model]                                        │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─ Hotkey ──────────────────────────────────────────────┐   │
│  │                                                        │  │
│  │  Global Hotkey     [  Super + Alt + Space  ] [Change]  │  │
│  │                                                        │  │
│  │  Double-tap to lock    [━━━●]                          │  │
│  │  Minimum hold time     [0.2s] ───●────── [1.0s]       │  │
│  │                                                        │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─ Behavior ────────────────────────────────────────────┐   │
│  │                                                        │  │
│  │  Auto-paste transcription   [━━━●]                     │  │
│  │  Sound effects              [━━━●]                     │  │
│  │  Start on login             [●━━━]                     │  │
│  │  Audio during recording     [Do nothing ▾]             │  │
│  │                                                        │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌─ Word Remapping ──────────────────────────────────────┐   │
│  │                                                        │  │
│  │  Find          →   Replace                             │  │
│  │  ┌──────────┐      ┌──────────┐                        │  │
│  │  │ I llama  │  →   │ I'll ama │   [✕]                  │  │
│  │  └──────────┘      └──────────┘                        │  │
│  │  ┌──────────┐      ┌──────────┐                        │  │
│  │  │ teh      │  →   │ the      │   [✕]                  │  │
│  │  └──────────┘      └──────────┘                        │  │
│  │                                                        │  │
│  │  [+ Add Rule]                                          │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                               │
└──────────────────────────────────────────────────────────────┘
```

**UI details:**
- **Dark by default** — matches the "always-on" nature of a background utility
- **Grouped sections** with subtle borders, not separate tabs — everything visible on one scroll
- **Hotkey capture:** clicking "Change" enters capture mode — next key combo is captured and displayed live
- **Toggle switches:** smooth 200ms CSS transitions, not instant snap
- **Model download:** inline progress bar replaces the "Downloaded" status area during download
- **Window size:** 520×auto (fixed width, content-driven height, max ~700px with scroll)

### 5.5 History Window

Searchable list of past transcriptions.

```
┌──────────────────────────────────────────────────────────────┐
│  📋 History                                         🔍 ─ □ ✕ │
│──────────────────────────────────────────────────────────────│
│  ┌────────────────────────────────────────────────────────┐  │
│  │  🔍  Search transcriptions...                          │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌────────────────────────────────────────────────────────┐  │
│  │  "The quick brown fox jumped over the lazy dog"        │  │
│  │  Today at 14:32 · 3.2s · Copied ✅                     │  │
│  │                                           [📋] [🗑️]   │  │
│  ├────────────────────────────────────────────────────────┤  │
│  │  "Remember to buy milk and eggs tomorrow"              │  │
│  │  Today at 13:15 · 2.1s · Pasted ✅                     │  │
│  │                                           [📋] [🗑️]   │  │
│  ├────────────────────────────────────────────────────────┤  │
│  │  "Meeting notes from the standup call"                 │  │
│  │  Yesterday at 09:05 · 8.4s · Pasted ✅                 │  │
│  │                                           [📋] [🗑️]   │  │
│  └────────────────────────────────────────────────────────┘  │
│                                                               │
│  [Clear All History]                                          │
└──────────────────────────────────────────────────────────────┘
```

**Features:**
- **Virtualized list** — render only visible items (1000 entries shouldn't stutter)
- **Search** — debounced 300ms, queries sidecar
- **Copy button** — copies text to clipboard immediately
- **Delete** — removes entry with slide-out animation
- **Relative timestamps** — "Just now", "5 min ago", "Yesterday at 14:30"
- **Click to expand** — long transcriptions are truncated with "…"
- **Empty state** — "No transcriptions yet. Press your hotkey and start talking! 🎤"

---

## 6. Performance Requirements

### 6.1 Startup & Responsiveness

| Metric | Target | Measurement |
|--------|--------|-------------|
| Cold start to tray icon visible | < 2s | `time` from process spawn to tray `ready` event |
| Recording overlay appears | < 50ms | From `RecordingStarted` event to window visible |
| Audio level update latency | < 16ms (60fps) | Time between `AudioLevel` event and visual update |
| Hotkey → recording start | < 100ms | End-to-end: key press → mic capture begins |
| Settings window open | < 200ms | Click → rendered |
| Sidecar idle CPU | < 0.5% | `top` while no recording is active |
| Sidecar idle RAM | < 30MB | Resident set size, no model loaded |
| Total app idle RAM (Electron + sidecar) | < 80MB | Sum of both processes |

### 6.2 Strategies

- **Pre-create overlay window** on app start, keep hidden. Show/hide is ~5ms vs ~200ms for creation.
- **Debounce audio levels** — sidecar sends at 20Hz (50ms interval), renderer batches with `requestAnimationFrame`.
- **Lazy-load settings/history windows** — don't create until first opened, then keep alive (hide, don't destroy).
- **Vite for renderer** — fast HMR in dev, tree-shaken production build.
- **No heavy JS in main process** — main process only does IPC relay and window management. All rendering in renderer.

### 6.3 Recording Pipeline Latency Budget

```
User releases hotkey
  → Sidecar detects release:         ~5ms   (evdev polling)
  → Stop audio capture:              ~1ms   (flag flip)
  → Play stop sound:                 ~5ms   (rodio, async)
  → Load ASR model (cached):         ~50ms  (first load) / ~5ms (warm)
  → Transcribe 3s audio:             ~100ms (Parakeet TDT, INT8, 4 threads)
  → Post-process:                    ~1ms   (word remapping)
  → Send event to Electron:          ~1ms   (stdout JSON)
  → Electron receives event:         ~1ms   (stdin readline)
  ─────────────────────────────────────────
  Total:                             ~165ms (first) / ~120ms (warm)
```

**Goal: user perceives text appearing "instantly" after they stop speaking.** At ~150ms total, the dominant perceptual delay is the stop sound + their own brain processing "I stopped talking". The transcription itself feels synchronous.

---

## 7. Platform-Specific Behavior

### 7.1 Global Hotkeys

| Platform | Method | Notes |
|----------|--------|-------|
| **Linux X11** | Sidecar's evdev/X11 listener (from `canario-core`) | Full press-and-hold + double-tap |
| **Linux Wayland** | Sidecar's evdev listener OR socket fallback | Same as GTK build |
| **macOS** | Electron's `globalShortcut` API | Press-and-hold needs custom logic (Electron only gives key-up/key-down) |
| **Windows** | Electron's `globalShortcut` API | Same as macOS |

**Important:** On macOS/Windows, the sidecar does NOT handle hotkeys. Electron's main process handles them and sends `toggle_recording` commands to the sidecar over IPC. This avoids the complexity of cross-platform evdev.

### 7.2 Auto-Paste

| Platform | Method |
|----------|--------|
| **Linux X11** | `xdotool type` (sidecar) |
| **Linux Wayland** | `wtype` / `ydotool` (sidecar) |
| **macOS** | Clipboard + simulated Cmd+V via Electron's `robotjs` or `nutjs` |
| **Windows** | Clipboard + simulated Ctrl+V via Electron's `robotjs` or `nutjs` |

**Key difference:** On macOS/Windows, auto-paste is handled in the Electron layer, not the sidecar. The sidecar still copies to clipboard (portable), but the "type into focused app" part uses platform-specific Electron APIs.

### 7.3 Sound Effects

| Platform | Method |
|----------|--------|
| **Linux** | Sidecar's `rodio` (canario-core `audio_effects`) |
| **macOS/Windows** | Electron's `shell.beep()` or HTML5 Audio in a hidden window |

For cross-platform consistency, the sidecar should play sounds on all platforms via `rodio` (which uses CoreAudio on macOS and WASAPI on Windows). No need for Electron-side audio.

---

## 8. UI Design System

### 8.1 Visual Language

- **Dark-first** — background utility apps shouldn't flash white. Default to dark, respect `prefers-color-scheme`.
- **Minimal chrome** — no toolbar, no sidebar. Content fills the window. One purpose per window.
- **System font** — `-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif`. No custom fonts to load.
- **Spacing:** 4px grid. 8px for small gaps, 16px for sections, 24px for page margins.
- **Border radius:** 8px for cards, 6px for inputs, 4px for buttons.

### 8.2 Color Palette (Dark Theme)

```
Background:     #1a1a2e  (deep navy-black)
Surface:        #16213e  (elevated cards)
Surface hover:  #1e2d4a
Border:         #2a3a5c
Text primary:   #e8e8f0
Text secondary: #8888a8
Accent:         #e94560  (canary red — brand color)
Accent hover:   #ff6b81
Success:        #4ade80  (green — pasted, downloaded)
Warning:        #fbbf24  (amber — downloading)
Error:          #ef4444  (red — error states)
Recording dot:  #ef4444  (pulsing red)
```

### 8.3 Component Library

Use **solid-ui** (SolidJS port of shadcn/ui, built on Kobalte + Tailwind) as the base component library. Reasons:
- Copy-paste components, no dependency lock-in (same philosophy as shadcn/ui)
- Kobalte provides accessible, headless primitives (Solid equivalent of Radix UI)
- Tailwind-based styling — easy to customize
- Dark mode built-in
- Same look and feel as shadcn/ui without the React dependency

Additional Solid-native packages:
- `@tanstack/solid-virtual` — virtualized history list (TanStack Virtual has a Solid adapter)
- `@solid-primitives/keyboard` — hotkey capture
- `@solid-primitives/timer` — elapsed timer in overlay

Custom components on top:
- `AudioLevel` — gradient bar with smooth interpolation
- `RecordingDot` — pulsing red circle (CSS keyframe animation)
- `HotkeyCapture` — keyboard event capture + display
- `Waveform` — (optional v2) real-time audio waveform visualization using Canvas

### 8.4 Animations

| Element | Animation | Duration | Easing |
|---------|-----------|----------|--------|
| Recording overlay appear | slide-down + fade | 150ms | ease-out |
| Recording overlay disappear | fade | 100ms | ease-in |
| Recording dot | pulse (scale 0.9→1.1, opacity 0.7→1.0) | 1.5s loop | ease-in-out |
| Audio level bar | width transition | 50ms | linear |
| Toggle switch | slide + color change | 200ms | ease-in-out |
| History item delete | slide-left + fade | 200ms | ease-in |
| Window open | fade + scale(0.95→1.0) | 150ms | ease-out |

---

## 9. Tech Stack

### 9.1 Rust Side (`canario-electron/`)

| Component | Choice | Why |
|-----------|--------|-----|
| Language | Rust | Same as canario-core |
| JSON parsing | `serde_json` | Already a dependency of core |
| Async runtime | `tokio` | Already a dependency (model download) |
| Build | `cargo build --release` | Static binary, no external deps |

### 9.2 Electron App (`canario-app/`)

| Component | Choice | Why |
|-----------|--------|-----|
| Electron | v33+ (latest stable) | Performance improvements, better macOS support |
| Renderer framework | **SolidJS** | Fine-grained reactivity — see §9.3 for full rationale |
| Build tool | Vite + `vite-plugin-solid` + electron-vite | Fast HMR, Solid JSX transform, tree-shaking |
| Styling | Tailwind CSS 4 | Utility-first, dark mode built-in, tiny bundle |
| Component library | **solid-ui** (Kobalte + Tailwind) | shadcn/ui port for Solid — same DX, no React |
| Type system | TypeScript 5 (strict) | Type safety for IPC protocol |
| Global state | **Custom state machine** (see §3.4) | Enforces valid transitions, no impossible states |
| Local state | **Solid primitives** (`createSignal`, `createStore`) | Fine-grained reactivity for component-level state |
| Packaging | electron-builder | Mature, cross-platform, auto-update |
| IPC types | Shared TypeScript types | Single source of truth for command/event shapes |

### 9.3 Why SolidJS Over React

SolidJS is a better fit for this project on every axis that matters:

#### Performance: fine-grained reactivity > VDOM diffing

This app's most performance-critical path is the **20Hz AudioLevel event stream** updating the recording overlay. Compare the two approaches:

**React** — `setState({level: 0.7})` triggers a VDOM diff of the entire component subtree containing the level bar. At 20fps that's 20 full subtree diffs per second. If the overlay component has 15 DOM nodes, React compares all 15 every 50ms. The recording dot animation, timer, and label are all re-reconciled even though only the bar width changed.

**SolidJS** — `setLevel(0.7)` updates exactly **one** DOM node's `style.width`. Zero VDOM. The signal was wired directly to that DOM node at setup time. The other 14 nodes in the overlay are never touched. This is fundamental — fine-grained reactivity means the audio level signal is bound to the bar's DOM node at compile time.

This matters for a background utility that must feel weightless. Every wasted CPU cycle is stealing from the user's foreground work.

#### Bundle size

```
SolidJS runtime:  ~7KB gzipped
React + ReactDOM: ~40KB gzipped
```

For a "get out of the way" background app, 33KB less JS to parse and execute on startup matters.

#### Simpler IPC integration

Solid's reactive primitives map naturally to the sidecar event stream. No hook rules, no stale closures, no dependency arrays:

```tsx
// React — useEffect with dep arrays, stale closure risk
function useCanario() {
  const [recording, setRecording] = useState(false);
  const [audioLevel, setAudioLevel] = createSignal(0);
  useEffect(() => {
    const unsub = window.electron.onEvent((e) => {
      if (e.event === "RecordingStarted") setRecording(true);
      if (e.event === "AudioLevel") setAudioLevel(e.level);
    });
    return unsub;
  }, []); // ← missing dep? stale closure. Wrong dep? infinite loop
}
```

```tsx
// Solid — runs once, auto-tracks, no stale closure possible
function createCanario() {
  const [recording, setRecording] = createSignal(false);
  const [audioLevel, setAudioLevel] = createSignal(0);

  // Runs once at setup, no dep array, no stale closure
  onCleanup(() => window.electron.removeAllListeners());
  window.electron.onEvent((e) => {
    if (e.event === "RecordingStarted") setRecording(true);
    if (e.event === "AudioLevel") setAudioLevel(e.level);
  });

  return { recording, audioLevel };
}
```

No `useCallback`, no `useMemo`, no `useRef` for mutable values, no rules-of-hooks lint rule.

#### Solid `Show` vs React conditional rendering

The recording overlay is pre-created and toggled with a visibility signal:

```tsx
// React — conditional rendering re-creates the DOM tree each time
return recording ? <RecordingOverlay /> : null;

// Solid — <Show> toggles visibility without destroying DOM
return <Show when={recording()}>
  <RecordingOverlay /> {/* created once, hidden/shown instantly */}
</Show>;
```

The overlay's CSS animations, canvas contexts, and DOM state survive visibility toggles — no re-initialization cost.

#### Component library: solid-ui

`solid-ui` is a direct port of shadcn/ui for SolidJS. It uses **Kobalte** (accessible headless primitives, equivalent to Radix UI) + Tailwind CSS. Same copy-paste model, same visual design, same customization approach. We get shadcn's design without React's weight.

Components we need (all available in solid-ui/Kobalte):
- Button, Switch, Input, Card, Progress, Dialog, Select, Toast

#### TypeScript

Solid has first-class TypeScript support. The JSX type system is actually stricter than React's (differentiates between DOM elements and components more precisely). Generic components work without the `extends React.FC` ceremony.

#### The one tradeoff

Solid's ecosystem is smaller than React's. But for a desktop app with a defined, limited UI (settings, tray, overlay, history, onboarding wizard), we don't need a vast ecosystem. We need ~10 well-built components — and we have them via solid-ui + Kobalte.

### 9.4 NOT Using

| Rejected | Why |
|----------|-----|
| React | SolidJS provides better performance for our 20Hz event stream, smaller bundle, simpler reactive model. No VDOM overhead. |
| Next.js / Remix | We're building a desktop app, not a website |
| Redux / Zustand / Jotai | The sidecar is the source of truth. A state machine coordinates global UI state. Solid signals handle the rest. No external lib needed. |
| Electron Forge | electron-builder is more mature for cross-platform packaging |
| Svelte | Good perf but weaker TypeScript support, Svelte-specific DSL instead of standard JSX |
| Vue | Larger runtime than Solid, Composition API is a half-measure vs Solid's true reactivity |
| Socket.io / WebSocket | stdin/stdout is simpler and faster for local IPC |
| XState / Robot | Our state graph is simple enough for a custom machine (see §3.4). No external dependency needed. |

---

## 10. Packaging & Distribution

### 10.1 Build Artifacts

| Platform | Format | Size (est.) | Notes |
|----------|--------|-------------|-------|
| **Linux** | AppImage | ~80MB | Self-contained, bundles Electron + Rust sidecar |
| **Linux** | .deb | ~70MB | Depends on system electron or bundles it |
| **macOS** | .dmg | ~90MB | Universal binary (arm64 + x64) if possible |
| **Windows** | .exe (NSIS) | ~85MB | Auto-update capable |

The Rust sidecar binary (~15-20MB static) is bundled inside the Electron package and extracted at runtime.

### 10.2 Auto-Update

- Use `electron-updater` with GitHub Releases as the update source
- Check for updates on launch + every 4 hours
- Download in background, prompt to restart
- Sidecar version must match Electron version — bundle them together

### 10.3 CI/CD

- GitHub Actions: build sidecar for linux-x64, macos-arm64, macos-x64, windows-x64
- Build Electron app with `electron-builder` using pre-built sidecar binaries
- Publish to GitHub Releases on tag

---

## 11. Development Phases

### Phase 0 — Foundation (PRD review + scaffolding)
**Goal:** Runnable skeleton with sidecar IPC

- [x] Create `canario-electron/` Rust crate with JSON stdin/stdout bridge
- [x] Add serde `Serialize` on `Event` enum (struct variants for clean JSON, backward-compatible)
- [x] Create `canario-app/` Electron project with electron-vite
- [x] Implement `sidecar.ts` — spawn process, parse JSON events
- [x] Implement `createCanario.ts` primitive — sidecar IPC bridge
- [x] Minimal renderer: model selector (v2/v3), record button, transcription display, history
- [x] Verify end-to-end: click record → recording → transcription displayed

**Exit criteria:** `npm run dev` → Electron app → click button → recording → transcription displayed

**Notes:**
- `Event` enum changed from tuple variants to struct variants (`Error { message }`, `AudioLevel { level }`, etc.) — all references updated across core/cli/gtk
- Preload must be CJS (`.cjs`) with `electron` kept external — Electron sandbox cannot run ESM imports
- `externalizeDepsPlugin()` must NOT bundle the npm `electron` package into preload
- Orphan process prevention: PPID watchdog in main process kills Electron + sidecar when parent dies
- `app.exit(0)` not `app.quit()` for forced shutdown (latter is async and can hang)

### Phase 1 — Core UI
**Goal:** Feature parity with GTK build

- [x] System tray icon with actual icon image (loaded from resources/icon.png)
- [x] System tray context menu (Start/Stop Recording, Settings, Quit)
- [x] Recording overlay window (pre-created, show/hide)
- [x] Audio level meter component with smooth animation
- [x] Verify overlay appears and animates during actual recording
- [x] Settings window — Model section (v2/v3 selector, download, delete, progress bar)
- [x] Settings window — Record section (mic button with record/stop states)
- [x] Settings window — History section (auto-loads on startup, displays entries)
- [x] Settings window — Hotkey capture widget
- [x] Settings window — Behavior toggles (auto-paste, sound effects, autostart)
- [x] Settings window — Word remapping section
- [x] History search and delete UI
- [x] Auto-paste on macOS/Windows (clipboard copy via Electron; auto-type deferred to Phase 3 with robotjs)
- [x] Dark theme CSS variables (light vars defined but no toggle UI)
- [x] Light theme toggle
- [x] Window state persistence (remember position/size)
- [x] Orphan process prevention (PPID watchdog + SIGTERM handler)
- [x] Recording overlay positioning (top-center, recalculated on show)
- [x] Sound effects integration (handled by sidecar via canario-core rodio; toggle in settings)

**Exit criteria:** Can daily-drive the Electron app instead of the GTK app on Linux

### Phase 2 — Polish
**Goal:** Make the existing UI feel refined and robust

- [x] Animations: overlay transitions, toggle switches, list items
- [x] Sound effects (sidecar's rodio on all platforms)
- [x] Autostart on login (macOS: LaunchAgent, Windows: registry, Linux: .desktop)
- [x] Error states with clear messages (no mic, no model, download failed)
- [x] Empty states with helpful copy

**Exit criteria:** App feels polished — smooth animations, clear error handling, no rough edges

### Phase 3 — Cross-Platform
**Goal:** Ship macOS and Windows builds

- [ ] Cross-compile Rust sidecar for macOS (arm64 + x64) and Windows (x64)
- [ ] macOS-specific: global shortcut via Electron API, paste via AppleScript/robotjs
- [ ] Windows-specific: global shortcut via Electron API, paste via robotjs
- [ ] Code signing (macOS: Apple Developer ID, Windows: certificate)
- [ ] Notarization (macOS)
- [ ] electron-builder configs for .dmg, .exe, AppImage
- [ ] GitHub Actions CI: build + test on all platforms

**Exit criteria:** Downloadable .dmg and .exe that work out of the box

### Phase 4 — Distribution & Auto-Update
**Goal:** Sustainable release pipeline

- [ ] Auto-update via electron-updater + GitHub Releases
- [ ] Version checking (sidecar + Electron must match)
- [ ] Update notifications (non-intrusive tray badge)
- [ ] GitHub Release automation on tag push
- [ ] Download page / landing page (can be a simple README section)
- [ ] Analytics opt-in (basic: daily active, transcription count, no text content)

**Exit criteria:** Push a git tag → CI builds → release published → users auto-update

---

## 12. Open Questions

| # | Question | Default Answer | Needs Discussion |
|---|----------|---------------|-----------------|
| 1 | Should `canario-core` add `serde::Serialize` on `Event`? | Yes — done. Struct variants with `#[serde(tag = "event")]` | ❌ settled |
| 2 | Should Electron share the same config file as GTK? | Yes — that's the point | ❌ settled |
| 3 | solid-ui (Kobalte + Tailwind) or custom components from scratch? | solid-ui — shadcn/ui port for Solid | ❌ settled |
| 4 | Separate repo or monorepo? | Same repo, `canario-app/` directory | ❌ settled |
| 5 | Auto-update in first release? | No — Phase 4 | ❌ settled |
| 6 | Should the overlay be a separate BrowserWindow or a BrowserView? | BrowserWindow — simpler API, separate process | ✅ perf test |
| 7 | Linux Wayland: use Electron's shortcut API or sidecar's evdev? | Sidecar's evdev (same as GTK) — more reliable | ❌ settled |
| 8 | macOS code signing: self-sign or paid Apple Developer? | Paid — required for distribution outside Xcode | ✅ budget |
| 9 | Custom state machine or XState? | Custom — 5 states don't justify a library | ❌ settled |

---

## 13. Success Metrics

| Metric | Target | How to measure |
|--------|--------|----------------|
| First transcription within onboarding | > 90% of new users | Onboarding step completion events |
| Recording overlay latency | < 50ms | Performance.now() in renderer |
| Total idle memory | < 80MB | Process monitor |
| User switches from GTK to Electron | > 50% of Linux users within 3 months | Download counts |
| Crash rate | < 0.1% of sessions | Sidecar exit codes |
| Settings adjusted after first week | < 30% of users | Config change events (if analytics added) |

---

## Appendix A: Sidecar Command Reference

Full list of commands the sidecar accepts, with their parameters and responses:

| Command | Params | Response `data` | Side Effects |
|---------|--------|-----------------|-------------|
| `start_recording` | — | — | Emits `RecordingStarted`, `AudioLevel` stream, then `TranscriptionReady` + `RecordingStopped` |
| `stop_recording` | — | — | Triggers transcription |
| `toggle_recording` | — | `{ recording: bool }` | Start or stop |
| `download_model` | — | — | Emits `ModelDownloadProgress`, then `Complete` or `Failed` |
| `delete_model` | — | — | Removes model files |
| `is_model_downloaded` | — | `bool` | — |
| `get_config` | — | `AppConfig` JSON | — |
| `update_config` | `config` (partial) | — | Merges and saves |
| `get_history` | `limit` | `[HistoryEntry]` | — |
| `search_history` | `query` | `[HistoryEntry]` | — |
| `delete_history` | `id` | — | Removes entry |
| `clear_history` | — | — | Removes all |
| `start_hotkey` | — | — | Emits `HotkeyTriggered` on hotkey |
| `stop_hotkey` | — | — | Stops listener |
| `restart_hotkey` | — | — | Reloads config + restarts |
| `ping` | — | `{ pong: true, version: "0.1.2" }` | Health check |
| `shutdown` | — | — | Stops recording + hotkey, exits |

## Appendix B: Event Reference

| Event | Fields | Frequency | UI Response |
|-------|--------|-----------|-------------|
| `RecordingStarted` | — | Once per recording | Show overlay, start dot animation |
| `RecordingStopped` | — | Once per recording | Hide overlay or change to "Transcribing…" |
| `TranscriptionReady` | `text`, `duration_secs` | Once per recording | Display text, auto-paste, add to history |
| `AudioLevel` | `level` (0.0–1.0) | ~20Hz during recording | Update level bar |
| `Error` | `message` | On failure | Show toast notification |
| `ModelDownloadProgress` | `progress` (0.0–1.0) | ~1Hz during download | Update progress bar |
| `ModelDownloadComplete` | — | Once | Update model status, enable recording |
| `ModelDownloadFailed` | `error` | Once | Show error with retry button |
| `HotkeyTriggered` | — | On hotkey press | Call `toggle_recording` |

---

*This PRD is a living document. Update it as decisions are made and scope evolves.*

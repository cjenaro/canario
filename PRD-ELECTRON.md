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
{"id":"11","cmd":"delete_history","entry_id":"uuid-here"}
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
{"event":"TranscriptionStarted"}
{"event":"TranscriptionReady","text":"hello world","duration_secs":3.2}
{"event":"AudioLevel","level":0.65}
{"event":"Error","message":"No microphone found"}
{"event":"ModelDownloadProgress","progress":0.42}
{"event":"ModelDownloadComplete"}
{"event":"ModelDownloadFailed","error":"Network timeout"}
{"event":"HotkeyTriggered"}
{"event":"ConfigChanged"}

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

> **Source of truth (canario-dmp.14):** `canario-app/src/renderer/state/types.ts` (`AppState`, `AppEvent`, `AppContext`, `transitions`) is the authoritative definition of this machine, and `canario-app/src/renderer/state/machine.ts` is its runtime. This section is maintained to match that code exactly — when they disagree, the code wins and this section must be updated.

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

```text
 START_ONBOARDING (App.tsx on first launch; AppPage Settings → About re-run)
   idle ─────────────────────▶ onboarding (step 1..3)
    ▲                              │     ▲
    │      WIZARD_COMPLETE         │     │ WIZARD_GOTO (guard: step 1..3) — self-loop
    │      (hasModel =             │     └────────────────────────────────┘
    │       ctx.modelReady)        │
    └──────────────────────────────┘     (WIZARD_COMPLETE is the only exit)

      START_RECORDING (guard: ctx.modelReady)      STOP_RECORDING
 idle ─────────────────────────▶ recording ─────────────────────▶ transcribing
  ▲                                  │                                 │
  │   RECORDING_CANCELLED / ERROR    │      TRANSCRIPTION_READY /      │
  │   (hasModel = ctx.modelReady)    │      RECORDING_STOPPED / ERROR  │
  └──────────────────────────────────┴─────────────────────────────────┘

      START_DOWNLOAD                      DOWNLOAD_COMPLETE (hasModel: true)
 idle ─────────────────▶ downloading ─────────────────────────────────▶ idle
                            │
                            ├──── DOWNLOAD_FAILED (hasModel: false) ──▶ idle
                            └──── DOWNLOAD_PROGRESS (self-loop, progress updated)

 From EVERY non-onboarding status (idle, recording, transcribing, downloading):
   SIDECAR_CRASHED ──▶ idle (hasModel = ctx.modelReady)   — force-idle, no zombie
   STATUS_SYNC     ──▶ core truth                          — reconciliation, see below
```

```typescript
// canario-app/src/renderer/state/types.ts — verbatim
export type AppState =
  | { status: "onboarding"; step: number }
  | { status: "idle"; hasModel: boolean }
  | { status: "recording"; startedAt: number }
  | { status: "transcribing"; startedAt: number }
  | { status: "downloading"; progress: number };
```

#### Events

```typescript
// canario-app/src/renderer/state/types.ts — verbatim
export type AppEvent =
  | { type: "START_ONBOARDING" }
  | { type: "WIZARD_GOTO"; step: number }
  | { type: "WIZARD_COMPLETE" }
  | { type: "START_RECORDING" }
  | { type: "STOP_RECORDING" }
  | { type: "START_DOWNLOAD" }
  | { type: "DOWNLOAD_PROGRESS"; progress: number }
  | { type: "DOWNLOAD_COMPLETE" }
  | { type: "DOWNLOAD_FAILED" }
  | { type: "TRANSCRIPTION_READY" }
  | { type: "RECORDING_STOPPED" }
  | { type: "RECORDING_CANCELLED" }
  | { type: "SIDECAR_CRASHED" }
  | { type: "STATUS_SYNC"; recording: boolean; transcribing: boolean; downloading: boolean }
  | { type: "ERROR" };
```

Who sends what (all through `createCanario.ts`, the IPC bridge):

| Event | Origin |
|-------|--------|
| `START_ONBOARDING` | `App.tsx` first-launch check (`getOnboardingCompleted() === false`); `AppPage` Settings → About re-run |
| `WIZARD_GOTO` / `WIZARD_COMPLETE` | `OnboardingPage` |
| `START_RECORDING` | sidecar `RecordingStarted` event, or an `ok` response from `start_recording` / the start leg of `toggle_recording` |
| `STOP_RECORDING` | an `ok` response from `stop_recording` / the stop leg of `toggle_recording` (see "Entry paths into `transcribing`" below) |
| `START_DOWNLOAD` | `canario.downloadModel()` (main settings page) |
| `DOWNLOAD_PROGRESS` / `DOWNLOAD_COMPLETE` / `DOWNLOAD_FAILED` | sidecar `ModelDownload*` events; `DOWNLOAD_FAILED` is also sent synthetically when the `download_model` command itself is rejected (no event will ever arrive) |
| `TRANSCRIPTION_READY` / `RECORDING_STOPPED` | sidecar `TranscriptionReady` / `RecordingStopped` events |
| `RECORDING_CANCELLED` | sidecar `RecordingCancelled` event (Escape-cancel) |
| `SIDECAR_CRASHED` | `SidecarCrashed` event synthesized by the Electron main process's sidecar manager when the process exits — not a core event |
| `STATUS_SYNC` | response to the sidecar's authoritative `status` command, sent on (re)mount |
| `ERROR` | sidecar `Error` event |

#### Transitions

The transition map defines which events are valid in each state. Anything not in this map is silently ignored — and a handler may also return `undefined` to reject the event while staying in the same state (that is how guards work).

```typescript
// canario-app/src/renderer/state/types.ts — verbatim (syncFromStatus included)

// Reconciliation from the sidecar's authoritative `status` command
// (canario-dmp.5): map core truth directly onto the machine, whatever
// the machine currently believes — it may have missed events across a
// reload. Download progress restarts at 0 and recovers on the next
// ModelDownloadProgress event.
function syncFromStatus(ctx: AppContext, event: AppEvent): AppState | undefined {
  if (event.type !== "STATUS_SYNC") return undefined;
  if (event.recording) return { status: "recording", startedAt: Date.now() };
  if (event.transcribing) return { status: "transcribing", startedAt: Date.now() };
  if (event.downloading) return { status: "downloading", progress: 0 };
  return { status: "idle", hasModel: ctx.modelReady };
}

export const transitions: TransitionMap = {
  onboarding: {
    // Absolute step navigation (1-3) — the component computes the target
    // step from the current state, keeping transition fns stateless.
    WIZARD_GOTO: (_ctx, event) => {
      if (event.type !== "WIZARD_GOTO") return undefined;
      if (event.step < 1 || event.step > 3) return undefined;
      return { status: "onboarding", step: event.step };
    },
    WIZARD_COMPLETE: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  idle: {
    START_ONBOARDING: () => ({ status: "onboarding", step: 1 }),
    START_RECORDING: (ctx) => {
      if (!ctx.modelReady) return undefined;
      return { status: "recording", startedAt: Date.now() };
    },
    START_DOWNLOAD: () => ({ status: "downloading", progress: 0 }),
    // Backend death is a state change even from idle: a fresh object
    // makes the signal notify watchers (e.g. the offline banner).
    SIDECAR_CRASHED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    STATUS_SYNC: syncFromStatus,
  },
  recording: {
    STOP_RECORDING: () => ({ status: "transcribing", startedAt: Date.now() }),
    // Escape-cancel from the core: audio discarded, no transcription/paste
    RECORDING_CANCELLED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    // No terminal core event can arrive anymore — force idle
    // (canario-dmp.6: no zombie recording state).
    SIDECAR_CRASHED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    STATUS_SYNC: syncFromStatus,
    ERROR: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  transcribing: {
    TRANSCRIPTION_READY: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    RECORDING_STOPPED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    SIDECAR_CRASHED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    STATUS_SYNC: syncFromStatus,
    ERROR: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  downloading: {
    DOWNLOAD_PROGRESS: (_ctx, event) => {
      const progress = event.type === "DOWNLOAD_PROGRESS" ? event.progress : 0;
      return { status: "downloading", progress };
    },
    DOWNLOAD_COMPLETE: () => ({ status: "idle", hasModel: true }),
    DOWNLOAD_FAILED: () => ({ status: "idle", hasModel: false }),
    // The download died with the process; readiness must be re-derived
    // after restart, so land on idle with the last known truth.
    SIDECAR_CRASHED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    STATUS_SYNC: syncFromStatus,
  },
};
```

**Guards.** A handler returning `undefined` rejects the event without leaving the state. Two guards exist:

- `idle + START_RECORDING` requires `ctx.modelReady` — you can't record without a model.
- `onboarding + WIZARD_GOTO` requires `step` 1..3 — the wizard can't navigate outside its three steps (the component computes the target step, keeping the transition stateless).

**Reconciliation: `STATUS_SYNC` (canario-dmp.5).** On (re)mount the IPC bridge asks the sidecar's authoritative `status` command (`{ recording, transcribing, downloading }`) and sends `STATUS_SYNC` — a settings-window reload resets this machine while core may be mid-recording or mid-download, and events fired before mount are gone forever. `STATUS_SYNC` is valid from **every non-onboarding status** and maps core truth directly onto the machine, whatever the machine currently believes. Precedence: `recording` → `transcribing` → `downloading` → `idle` (`hasModel: ctx.modelReady`); a reconciled download restarts at `progress: 0` and recovers on the next `ModelDownloadProgress` event. It is deliberately **not** valid from `onboarding` — the wizard owns its own flow (§5.1, "Onboarding download path").

**Crash safety: `SIDECAR_CRASHED` (canario-dmp.6).** When the backend process dies, no terminal core event will ever arrive — `recording`/`transcribing` would be zombies and `downloading` would spin forever. So `SIDECAR_CRASHED` forces any active status back to `idle` with `hasModel: ctx.modelReady`. It is also valid from `idle`, where it emits a fresh state object so the Solid signal notifies watchers (the offline banner).

**Escape-cancel: `RECORDING_CANCELLED`.** The core's Escape-cancel discards the audio — no transcription, no paste, no history — so `recording` returns straight to `idle`; no `TranscriptionReady` follows.

**Entry paths into `transcribing` — there are three:**

1. **Successful stop-response sniff.** `createCanario.stopRecording()` sends `STOP_RECORDING` only when the `stop_recording` command response is `ok`; `toggleRecording()` sends it when the `toggle_recording` response reports `recording: false`. The command response — not an event — is the confirmation that core accepted the stop and is about to transcribe.
2. **The `TranscriptionStarted` event.** Core emits it when the finished capture actually begins transcribing (`canario-core/src/event.rs`); the renderer treats it as an idempotent belt-and-braces `STOP_RECORDING`, and the Electron main process uses it to push the overlay's "transcribing" state (canario-dmp.9).
3. **`STATUS_SYNC` with `transcribing: true`.** Reconciliation on (re)mount maps core truth directly into `transcribing` (e.g. after a reload that ate the stop events).

The response sniff usually wins the race (the response is written before the recording thread starts transcribing), which is why both paths are documented as canonical — the event is the robust one for consumers that issue no stop command themselves.

#### Context (extended state)

The machine carries a context object alongside the state. This is data that persists across transitions but doesn't define the state itself:

```typescript
// canario-app/src/renderer/state/types.ts — verbatim
export type AppContext = {
  modelReady: boolean;                     // is the ASR model downloaded?
  lastTranscription: string | null;
  lastError: string | null;
  lastDuration: number | null;             // length (secs) of the last transcription
  config: Record<string, unknown> | null;  // snapshot of sidecar config
};
```

Context is updated in two places, never by components directly: `send()` applies `modelReady: true`/`false` on `DOWNLOAD_COMPLETE`/`DOWNLOAD_FAILED`, and the IPC bridge (`createCanario.ts`) writes everything else — `lastTranscription` + `lastDuration` on `TranscriptionReady`, `lastError` on `Error`/`ModelDownloadFailed`/`SidecarCrashed`, refreshed `modelReady` from `checkModel()` (`is_model_downloaded`), and the `config` snapshot — via `machine.updateContext()`. (`TRANSCRIPTION_READY` and `ERROR` intentionally update context *outside* `send()`.) Context is read by components through the same Solid context as the state. It's not a separate store — it lives inside the machine.

#### Implementation: custom, no library

Two files, ~170 lines total: `types.ts` (states, events, context, transition map — the spec) and `machine.ts` (the runtime):

```typescript
// src/renderer/state/machine.ts — verbatim
import { createSignal } from "solid-js";
import type { AppState, AppEvent, AppContext } from "./types";
import { transitions, defaultContext } from "./types";

export function createAppMachine() {
  const [state, setState] = createSignal<AppState>({
    status: "idle",
    hasModel: false,
  });
  const [context, setContext] = createSignal<AppContext>(defaultContext);

  function send(event: AppEvent) {
    const current = state();
    const status = current.status;
    const ctx = context();

    const handler = transitions[status]?.[event.type];
    if (!handler) return; // ignore invalid transitions

    const next = handler(ctx, event);
    if (!next) return; // guard rejected

    setState(next);

    // Update context based on event
    setContext((prev) => {
      const update = { ...prev };
      switch (event.type) {
        case "TRANSCRIPTION_READY":
          // Updated externally by the IPC bridge
          break;
        case "DOWNLOAD_COMPLETE":
          update.modelReady = true;
          break;
        case "DOWNLOAD_FAILED":
          update.modelReady = false;
          break;
        case "ERROR":
          // Error is set externally
          break;
      }
      return update;
    });
  }

  function updateContext(partial: Partial<AppContext>) {
    setContext((prev) => ({ ...prev, ...partial }));
  }

  return { state, context, send, updateContext };
}
```

Note there is **no** `event.contextUpdate` payload and no effect machinery — the machine starts in `idle`, enters `onboarding` via `START_ONBOARDING` (App.tsx's first-launch check), and all other context writes go through `updateContext()` from the IPC bridge. Solid signals make it reactive — any component that reads `state()` or `context()` automatically updates when the machine transitions.

#### How components use it

```tsx
// Illustrative — how a machine-driven component reads status. (The real
// overlay deliberately does NOT do this: OverlayPage.tsx is self-contained
// and listens to sidecar events directly, bypassing the modelReady guard —
// see §5.3.)
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
// AppPage.tsx (settings) — model download goes through the bridge
function ModelSection() {
  const { state } = useAppState();
  const canario = useCanario(); // createCanario(machine)

  return (
    <Show when={state().status === "downloading"} fallback={
      <Button onClick={() => canario.downloadModel()}>Download Model</Button>
    }>
      <Progress value={state().progress} />
    </Show>
  );
}
```

`canario.downloadModel()` sends `START_DOWNLOAD` and then issues the `download_model` command; if the command itself is rejected it sends a synthetic `DOWNLOAD_FAILED` so the machine can't wedge on a 0% bar (the real AppPage also keeps per-variant readiness via `onModelDownloadComplete`).

```tsx
// App.tsx — routing on machine status
<Show when={machine.state().status === "onboarding"} fallback={<AppPage />}>
  <OnboardingPage />
</Show>
```

No component ever checks `if (isRecording && !isDownloading && modelReady)`. The machine already guarantees it. Components just match on `state().status` and render.

#### Why not XState / Robot / another library?

- Our state graph has **5 states**, **15 event types**, and **22 transitions** (8 of which are the shared `SIDECAR_CRASHED`/`STATUS_SYNC` safety edges — four each). XState is designed for complex machines with hundreds of states, parallel regions, hierarchical composition. It would add ~15KB for a problem we solve in ~170 lines across two files.
- `robot` is lighter but still an unnecessary dependency for this graph size.
- A custom machine built on Solid signals is: zero dependencies, fully typed, reactive by default, auditable in two small files (`types.ts` + `machine.ts`), and debuggable with a `console.log` in `send()`.
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
│   │   │   ├── HotkeyCapture.tsx  # keyboard capture widget
│   │   │   ├── Toggle.tsx         # toggle switch component
│   │   │   ├── WordRemapping.tsx  # word remapping rules editor
│   │   │   └── Toast.tsx          # toast notification system
│   │   ├── pages/
│   │   │   ├── OverlayPage.tsx   # recording overlay (self-contained, direct sidecar events)
│   │   │   ├── AppPage.tsx       # main settings page
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

#### Onboarding download path (decided)

**Decision (canario-dmp.15): the split between wizard-local download tracking and the machine's `downloading` status is deliberate, not drift.** During the wizard the state machine parks in `onboarding` (step 1–3), where none of its download transitions are defined — `START_DOWNLOAD`, `DOWNLOAD_PROGRESS`, `DOWNLOAD_COMPLETE`, `DOWNLOAD_FAILED` (and `STATUS_SYNC`) are all absent from the `onboarding` row of the transition map — so the machine-level event handler in `createCanario.ts` is intentionally inert for download events while onboarding. The wizard owns the download UX instead: `OnboardingPage` starts the download with a raw `download_model` **command** (deliberately not the `downloadModel()` wrapper, which is what sends `START_DOWNLOAD`) and tracks it in wizard-local state — `dlProgress` plus a speed/ETA estimator (`dlStats`) — fed by its **own** `api.onEvent` listener for `ModelDownloadProgress` / `ModelDownloadComplete` / `ModelDownloadFailed`. The machine's `downloading` status is only ever entered from `idle` (the main Settings → Model page) or by `STATUS_SYNC` reconciliation after a reload — never from `onboarding`. This isn't just preference: `App.tsx` routes on `status === "onboarding"`, so any download transition out of `onboarding` would unmount the wizard mid-download.

Consequences of the decision (all visible in the code):

- **The wizard must handle `ModelDownloadFailed` itself.** Its listener clears `dlProgress` (restoring the Download button); the error toast still comes via `context.lastError` — the machine-level listener records it (`updateContext` works from any status, only *transitions* are inert), and the wizard's `createEffect` on `lastError` shows the toast.
- **A cancel mid-onboarding keeps `.part` files for resume.** The wizard's Cancel button sends `cancel_download` directly; the sidecar follows with `ModelDownloadFailed` (handled by the wizard listener as above), and partial files are kept so the next download resumes where this one stopped. The wizard shows its own "Download cancelled — it will resume next time" toast.
- **After `WIZARD_COMPLETE`, readiness lands in `hasModel` via `ctx.modelReady`.** Context stays live even while transitions are inert: the machine-level listener calls `checkModel()` on `ModelDownloadComplete` / `ModelDownloadFailed`, and the wizard calls it on mount and on model selection — so `ctx.modelReady` is accurate when `WIZARD_COMPLETE` maps it into `{ status: "idle", hasModel: ctx.modelReady }`.

The same parking applies to the step-1 mic test: the wizard issues `start_recording` / `stop_recording` commands directly and swallows the expected "no model" transcription error, while the machine stays in `onboarding` throughout.

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

The signature visual moment. A compact pill-shaped indicator that appears the instant recording starts, floating at the top-center of the screen.

```
         ┌────────────────────────────┐
         │ ● ▎▌█▌▎  0:03            │
         └────────────────────────────┘
```

**Spec:**
- **Shape:** Compact pill (`rounded-full`), ~180×32px
- **Position:** Top-center of screen, 10–12px from top edge
- **Appearance:** rounded-full pill, semi-transparent dark background (`rgba(26, 26, 46, 0.92)`), backdrop-blur, accent-colored border
- **Always on top** — `alwaysOnTop: true, focusable: false`
- **Click-through** — `setIgnoreMouseEvents(true)` so the overlay doesn't block windows below
- **Content (while recording):**
  - Pulsing red dot (CSS animation, 1.5s cycle)
  - 5-bar waveform visualization driven by audio levels — bars grow/brighten when audio is detected, stay small/dim when silent (gives immediate visual feedback that audio is being captured)
  - Elapsed timer (M:SS)
- **Transition in:** slide-down + fade (150ms ease-out)
- **Transition out:** disappears instantly when the user releases the hotkey (no delay)
- **Disappears** on `RecordingStopped`, `TranscriptionReady`, or `Error` — no lingering

**Implementation notes:**
- The overlay is a **full-screen transparent BrowserWindow** (`width × height` of the display), anchored at `(0, 0)`. CSS positions the pill at `fixed inset-0 flex items-start justify-center pt-3`. This approach works on all platforms including Linux/Wayland where `setPosition()` is unreliable.
- The overlay page (`OverlayPage.tsx`) is **self-contained** — it listens to sidecar events directly via the preload API and manages its own local state. It does **not** use the global state machine (which has a `modelReady` guard that would block transitions in the overlay's fresh context).
- `setIgnoreMouseEvents(true)` on the overlay window ensures clicks pass through to windows below.
- Audio level is smoothed with exponential moving average (`0.6/0.4` blend) to avoid jitter.
- Waveform bars animate via `requestAnimationFrame` for smooth 60fps updates independent of IPC event rate.

**Performance constraints:**
- Window must appear within **50ms** of `RecordingStarted` event
- Audio level updates must not drop frames — use `requestAnimationFrame` for the waveform bars, decouple from IPC event rate
- Window creation: **pre-create** the overlay window on app start, hide it. Show/hide is near-instant. Don't create on demand.
- Overlay hides instantly on hotkey release — `toggleRecording()` calls `api.hideOverlay()` immediately, does not wait for sidecar's `RecordingStopped` event.

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
- **Full-screen transparent overlay** — the overlay window covers the entire display. CSS handles pill positioning at top-center. This avoids `setPosition()` issues on Linux/Wayland where the window manager ignores window placement. `setIgnoreMouseEvents(true)` ensures the overlay doesn't intercept clicks.
- **Smooth audio levels** — sidecar sends at 20Hz (50ms interval), renderer smooths with exponential moving average and animates waveform bars via `requestAnimationFrame`.
- **Instant overlay hide** — `toggleRecording()` calls `hideOverlay()` immediately when recording stops, without waiting for the sidecar's asynchronous `RecordingStopped` event.
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
| **macOS** | Clipboard + simulated Cmd+V via `@jitsi/robotjs` (Electron main process) |
| **Windows** | Clipboard + simulated Ctrl+V via `@jitsi/robotjs` (Electron main process) |

**Key difference:** On macOS/Windows, auto-paste is handled in the Electron layer using `@jitsi/robotjs` (Jitsi-maintained native addon). The sidecar still copies to clipboard (portable), but the "type into focused app" part uses robotjs. On macOS, Accessibility permissions are required — the app prompts the user on first paste attempt.

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
| Recording overlay disappear | instant (no animation) | 0ms | — |
| Recording dot | pulse (scale 0.9→1.1, opacity 0.7→1.0) | 1.5s loop | ease-in-out |
| Waveform bars | height + color transition | 80ms | ease-out |
| Audio glow | box-shadow pulse | 200ms | ease |
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
- [x] Recording overlay redesign: compact pill shape with 5-bar waveform audio indicator, full-screen transparent window for Wayland compat, click-through, instant hide on release
- [x] Overlay self-contained event listener (no state machine dependency — avoids modelReady guard blocking transitions)
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

- [x] Cross-compile Rust sidecar for macOS (arm64 + x64) and Windows (x64)
- [x] macOS-specific: global shortcut via Electron API, paste via robotjs
- [x] Windows-specific: global shortcut via Electron API, paste via robotjs
- [ ] Code signing (macOS: Apple Developer ID, Windows: certificate)
- [ ] Notarization (macOS)
- [x] electron-builder configs for .dmg, .exe, AppImage
- [x] GitHub Actions CI: build + test on all platforms

**Notes:**
- Code signing and notarization deferred — unsigned builds work with manual approval (right-click → Open on macOS, More info → Run anyway on Windows)
- Auto-paste on macOS/Windows uses `@jitsi/robotjs` (Jitsi-maintained native addon). On first paste attempt without Accessibility permissions, the user is prompted.
- macOS: app hides from Dock (tray-only). Shows in Dock when Settings window is opened.
- Sidecar path resolution handles Windows `.exe` extension automatically.
- Config cache in main process keeps `auto_paste` flag in sync between renderer and main process for cross-platform auto-paste.

**Exit criteria:** Downloadable .dmg and .exe that work out of the box

### Phase 4 — Distribution & Auto-Update
**Goal:** Sustainable release pipeline

- [x] Auto-update via electron-updater + GitHub Releases
- [x] Version checking (sidecar + Electron must match)
- [x] Update notifications (non-intrusive system notification + in-app)
- [x] GitHub Release automation on tag push
- [x] Download page / landing page (README section)

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
| Settings adjusted after first week | < 30% of users | Config change events |

---

## Appendix A: Sidecar Command Reference

Full list of commands the sidecar accepts, with their parameters and responses:

| Command | Params | Response `data` | Side Effects |
|---------|--------|-----------------|-------------|
| `start_recording` | — | — | Emits `RecordingStarted`, `AudioLevel` stream, then `TranscriptionReady` + `RecordingStopped` |
| `stop_recording` | — | — | Triggers transcription |
| `toggle_recording` | — | `{ recording: bool }` | Start or stop |
| `cancel_recording` | — | — | Discards the in-flight recording: no transcription, no paste, no history entry; emits `RecordingCancelled`. Safe no-op when idle (canario-dmp.5) |
| `cancel_download` | — | — | Requests download cancellation; `ModelDownloadFailed` follows and `.part` files are kept for resume. Safe no-op when idle |
| `is_downloading` | — | `bool` | Authoritative "download in flight" — events alone can't answer after a reload |
| `status` | — | `{ recording: bool, transcribing: bool, downloading: bool }` | Lifecycle truth for machine reconciliation on (re)mount |
| `download_model` | — | — | Emits `ModelDownloadProgress`, then `Complete` or `Failed` |
| `delete_model` | — | — | Removes model files |
| `is_model_downloaded` | — | `bool` | — |
| `get_config` | — | `AppConfig` JSON | — |
| `update_config` | `config` (partial) | — | Merges and saves |
| `get_history` | `limit` | `[HistoryEntry]` | — |
| `search_history` | `query` | `[HistoryEntry]` | — |
| `delete_history` | `entry_id` (canonical; `target_id` accepted as an alias) | — | Removes entry. Neither present → `ok:false` error naming `entry_id` — the request `id` never doubles as the entry id (canario-dmp.8) |
| `clear_history` | — | — | Removes all |
| `start_hotkey` | — | — | Emits `HotkeyTriggered` on hotkey |
| `stop_hotkey` | — | — | Stops listener |
| `restart_hotkey` | — | — | Reloads config + restarts |
| `hotkey_status` | — | `HotkeyStatus` | Hotkey backend health; Linux evdev permission failures carry `fix_command` |
| `paste_text` | `text` | `{ pasted: bool }` | Native paste chord into the focused window (Linux xdotool/wtype/ydotool, macOS CoreGraphics, Windows SendInput — canario-7x5.3). The frontend owns the clipboard write + verified read-back and calls this for the chord; `pasted: false` (not an error) when no backend delivers — the text is already on the clipboard for a manual paste (canario-ubb) |
| `set_transform_credential` | `key` (string or null) | `{ stored: bool }` | Stores (non-empty) or drops (null/blank) the transform provider API key in the sidecar's **memory only** — never config.json, never logs (canario-fgm.2; raw credential-bearing lines are withheld/redacted on every log path). The Electron main process persists the key via safeStorage and pushes it here at boot and on change |
| `transform_status` | — | `{ enabled, provider: { base_url, model }, timeout_ms, credential_present }` | Sidecar truth about the transformation feature — the provider block from (reloaded) config plus whether the memory-only credential is held. The key itself never crosses the wire |
| `transform_test` | — | `{ latency_ms }` or error | One minimal chat-completions round trip through the configured provider with the in-memory credential (the settings "Test connection" button). Fails fast with a descriptive error; payload is transcript+instruction only (fgm.1 D5) |
| `set_autostart` | `enabled`, `exec` (optional) | `{ enabled: bool }` | Creates/removes the single login entry (`~/.config/autostart/com.canario.Canario.desktop`) and keeps `config.autostart` in sync; `exec` writes a standalone entry, omit it to symlink the menu entry |
| `ping` | — | `{ pong: true, version: "0.1.2", protocol: 1 }` | Health check + protocol handshake. `protocol` is the wire-compatibility version (`PROTOCOL_VERSION`, pinned in lockstep with canario-app/src/main/version.ts by the sidecar's protocol tests); a mismatching or missing number makes the app show a persistent version-mismatch warning (canario-dmp.4). Stays 1 — nothing has shipped since it was introduced, and the additive events plus the `delete_history` `entry_id` fix ride it |
| `diagnostics` | — | `Diagnostics` JSON (see below) | Reads log tail, probes tools |
| `shutdown` | — | — | Stops recording + hotkey, exits |

`diagnostics` returns a snapshot for support bundles / the "Copy diagnostics"
button. Shape (all best-effort, fields degrade to `null`/`false`):

```json
{
  "core_version": "0.1.2",
  "frontend": { "name": "canario-electron", "version": "0.1.2" },
  "system": { "os": "linux", "arch": "x86_64", "kernel": "Linux 6.x", "display_server": "wayland" },
  "config_path": "~/.config/canario/config.json",
  "config": { "...": "full AppConfig (local only, nothing redacted)" },
  "model": {
    "variant": "ParakeetV3",
    "downloaded": true,
    "paths_error": null,
    "files": [{ "path": "…/encoder.int8.onnx", "exists": true, "size_bytes": 123 }]
  },
  "tools": { "xdotool": true, "wtype": false, "ydotool": false, "pactl": true },
  "logs": {
    "dir": "~/.local/state/canario/logs",
    "latest_file": "~/.local/state/canario/logs/canario.log.2026-09-10",
    "tail": ["last ~50 log lines"]
  }
}
```

The same blob is available on the CLI via `canario-cli --diagnostics`
(frontend name `canario-cli`).

## Appendix B: Event Reference

| Event | Fields | Frequency | UI Response |
|-------|--------|-----------|-------------|
| `RecordingStarted` | — | Once per recording | Show overlay, start dot animation |
| `RecordingStopped` | — | Once per recording | Hide overlay or change to "Transcribing…" |
| `RecordingCancelled` | — | On Escape-cancel | Audio was discarded — hide recording UI, no paste, no history |
| `TranscriptionStarted` | — | Once per recording | Emitted when the finished capture begins transcribing (before decode starts) — show the "Transcribing…" state; deriving the state from a successful `stop_recording` response is equally valid, both paths are canonical (canario-dmp.9) |
| `TranscriptionReady` | `text`, `duration_secs`; optional `raw_text`, `transform_failed` | Once per recording | Display text, auto-paste, add to history. `text` is canonical — the transformed transcript when `transform.enabled` ran a pass (fgm.3 D3: the pipeline completes before this event). `raw_text` (pre-transform transcript) rides along only when a transformation changed the text; `transform_failed` is true when a pass was attempted and failed/timed out and the raw text flowed on (D5d) — frontends surface the fallback affordance. Both fields are wire-skipped when unset, so pre-fgm.3 frontends see the exact old shape |
| `AudioLevel` | `level` (0.0–1.0) | ~20Hz during recording | Update level bar |
| `PartialTranscript` | `text` | Periodic during long recordings (after the live-captions threshold) | Live preview only — never paste or store; the authoritative text arrives via `TranscriptionReady` |
| `Error` | `message` | On failure | Show toast notification |
| `ModelDownloadProgress` | `progress` (0.0–1.0) | ~1Hz during download | Update progress bar |
| `ModelDownloadComplete` | — | Once | Update model status, enable recording |
| `ModelDownloadFailed` | `error` | Once | Show error with retry button |
| `HotkeyTriggered` | — | On hotkey press | Call `toggle_recording` |
| `ConfigChanged` | — | On any config change | Emitted after any config.json write by this instance, or when a reload detects an external change; payload-free — consumers pull `get_config` (canario-dmp.20) |

---

*This PRD is a living document. Update it as decisions are made and scope evolves.*

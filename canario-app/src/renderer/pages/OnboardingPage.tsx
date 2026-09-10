// Onboarding wizard — PRD §5.1
// 3 steps: Download Model → Set Hotkey → Ready (practice area)
// Reached when the state machine is in `onboarding` (see App.tsx routing).
import { createSignal, Show, For, onMount, onCleanup, createEffect } from "solid-js";
import { useAppState } from "../state/context";
import { createCanario } from "../primitives/createCanario";
import { HotkeyCapture, toAccelerator } from "../components/HotkeyCapture";
import { Toggle } from "../components/Toggle";
import { ToastContainer, showToast } from "../components/Toast";
import { applyTheme } from "../theme";

const MODELS = [
  { id: "ParakeetV3", name: "Parakeet TDT v3", desc: "Multilingual · ~640MB" },
  { id: "ParakeetV2", name: "Parakeet TDT v2", desc: "English only · ~640MB" },
] as const;

const MODEL_SIZE_MB = 640;
const STEP_LABELS = ["Download Model", "Set Hotkey", "Ready"];
const MIC_TEST_MS = 3000;

function formatEta(seconds: number): string {
  if (!isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return `${Math.ceil(seconds)}s`;
  return `${Math.floor(seconds / 60)}m ${Math.ceil(seconds % 60)}s`;
}

export function OnboardingPage() {
  const machine = useAppState();
  const canario = createCanario(machine);
  const { state, context, send, updateContext } = machine;

  // Current wizard step comes from the state machine
  const step = () => {
    const s = state();
    return s.status === "onboarding" ? s.step : 1;
  };

  const [selectedModel, setSelectedModel] = createSignal<string>("ParakeetV3");
  const [modelReady, setModelReady] = createSignal(false);
  const [dlProgress, setDlProgress] = createSignal<number | null>(null);
  const [level, setLevel] = createSignal(0);
  const [micTesting, setMicTesting] = createSignal(false);
  const [hotkey, setHotkey] = createSignal<string[]>([]);
  const [autostart, setAutostart] = createSignal(false);
  const [platform, setPlatform] = createSignal({ isLinux: true, isMac: false, isWindows: false });

  let practiceRef: HTMLTextAreaElement | undefined;
  let micTestTimer: ReturnType<typeof setTimeout> | undefined;
  // Suppress the error toast for the expected "no model" transcription
  // failure when the mic test stops recording without a model installed.
  let suppressErrorsUntil = 0;

  // Download speed/ETA estimation (sidecar only emits 0..1 progress)
  let lastP = 0;
  let lastT = 0;
  let speedEma = 0; // MB/s

  function handleProgress(p: number) {
    setDlProgress(p);
    const now = Date.now();
    if (lastT === 0) {
      lastP = p;
      lastT = now;
      return;
    }
    const dt = (now - lastT) / 1000;
    if (dt >= 0.25 && p > lastP) {
      const inst = ((p - lastP) * MODEL_SIZE_MB) / dt;
      speedEma = speedEma === 0 ? inst : speedEma * 0.7 + inst * 0.3;
      lastP = p;
      lastT = now;
    }
  }

  const dlStats = () => {
    const p = dlProgress();
    if (p === null) return "";
    const done = Math.round(p * MODEL_SIZE_MB);
    if (speedEma > 0.1) {
      const eta = formatEta(((1 - p) * MODEL_SIZE_MB) / speedEma);
      return `${done} / ${MODEL_SIZE_MB} MB · ${speedEma.toFixed(1)} MB/s${eta ? ` · ~${eta} left` : ""}`;
    }
    return `${done} / ${MODEL_SIZE_MB} MB`;
  };

  // Surface sidecar errors as toasts (mirrors AppPage), except during mic test
  createEffect(() => {
    const err = context().lastError;
    if (err) {
      if (Date.now() >= suppressErrorsUntil) {
        showToast(err, "error", 6000);
      }
      updateContext({ lastError: null });
    }
  });

  // Autofocus the practice area when step 3 is reached
  createEffect(() => {
    if (step() === 3) {
      setTimeout(() => practiceRef?.focus(), 100);
    }
  });

  async function handleSelectModel(modelId: string) {
    setSelectedModel(modelId);
    await canario.updateConfig({ model: modelId });
    setModelReady(await canario.checkModel());
  }

  async function handleDownload() {
    lastP = 0;
    lastT = 0;
    speedEma = 0;
    setDlProgress(0);
    await canario.command("download_model");
  }

  async function startMicTest() {
    if (micTesting()) return;
    const res = await canario.command("start_recording");
    if (!res?.ok) {
      showToast("Could not access the microphone. Check your audio settings.", "error", 6000);
      return;
    }
    setMicTesting(true);
    micTestTimer = setTimeout(() => stopMicTest(), MIC_TEST_MS);
  }

  async function stopMicTest() {
    if (!micTesting()) return;
    clearTimeout(micTestTimer);
    // Stopping without a model triggers a transcription error — swallow it
    suppressErrorsUntil = Date.now() + 2000;
    await canario.command("stop_recording");
    setMicTesting(false);
    setLevel(0);
  }

  async function handleHotkeyChange(keys: string[]) {
    setHotkey(keys);
    await canario.updateConfig({ hotkey: keys });
    if (platform().isLinux) {
      await canario.restartHotkey();
    } else if (keys.length > 0) {
      await canario.registerShortcut(toAccelerator(keys));
    }
  }

  async function handleAutostart(value: boolean) {
    setAutostart(value);
    await canario.updateConfig({ autostart: value });
    const ok = await canario.setAutostart(value);
    if (!ok) {
      showToast("Could not change autostart setting.", "warning");
      setAutostart(!value);
    }
  }

  function gotoStep(n: number) {
    send({ type: "WIZARD_GOTO", step: n });
  }

  // Mark onboarding complete and return to normal flow.
  // Per PRD §5.1 step 3, "Done" minimizes the app to the tray.
  async function completeWizard(minimizeToTray: boolean) {
    await stopMicTest();
    await canario.setOnboardingCompleted(true);
    send({ type: "WIZARD_COMPLETE" });
    if (minimizeToTray) {
      await canario.hideSettings();
    }
  }

  // Insert a transcription into the practice textarea at the caret. The
  // app already HAS the text — never depend on the main process's
  // simulated Ctrl+V, which pastes whatever the compositor clipboard
  // holds if Electron's clipboard write hasn't propagated yet (stale-
  // content paste, canario-fhm).
  function fillPracticeArea(text: string) {
    const el = practiceRef;
    if (!el || !text) return;
    const atEnd =
      el.selectionStart === el.selectionEnd && el.selectionStart === el.value.length;
    if (atEnd) {
      el.value = el.value ? `${el.value} ${text}` : text;
      el.selectionStart = el.selectionEnd = el.value.length;
    } else {
      el.setRangeText(text, el.selectionStart, el.selectionEnd, "end");
    }
    el.focus();
  }

  onMount(async () => {
    try {
      applyTheme(await canario.getTheme());

      const p = await canario.getPlatform();
      if (p) setPlatform(p);

      const cfg = (await canario.getConfig()) as Record<string, unknown> | undefined;
      if (cfg) {
        const modelId = (cfg.model as string) || "ParakeetV3";
        setSelectedModel(modelId);
        setHotkey((cfg.hotkey as string[]) || []);
        setAutostart((cfg.autostart as boolean) ?? false);
      }

      setModelReady(await canario.checkModel());
    } catch (err) {
      console.error("[OnboardingPage] init error:", err);
      showToast("Failed to initialize. Check that the canario sidecar is running.", "error", 8000);
    }

    // Wizard-local sidecar events: live download progress + mic levels.
    // (The machine-level listener in createCanario ignores these while in
    // the `onboarding` state, so the wizard tracks them itself.)
    const api = window.canario;
    if (api) {
      const unsub = api.onEvent((event) => {
        switch (event.event as string) {
          case "ModelDownloadProgress":
            handleProgress(event.progress as number);
            break;
          case "ModelDownloadComplete":
            setDlProgress(null);
            setModelReady(true);
            showToast("Model downloaded — you're good to go!", "success", 3000);
            break;
          case "ModelDownloadFailed":
            setDlProgress(null);
            break; // error toast handled via context.lastError
          case "AudioLevel":
            if (micTesting()) setLevel(event.level as number);
            break;
          case "TranscriptionReady":
            // Practice area: fill directly from the event — the main
            // process skips its simulated paste whenever one of our own
            // windows is focused, so this is the only writer.
            fillPracticeArea((event.text as string ?? "").trim());
            break;
        }
      });
      onCleanup(unsub);
    }
  });

  onCleanup(() => clearTimeout(micTestTimer));

  // Reusable styles (same as AppPage)
  const sectionStyle = { "background-color": "var(--surface)", "border-color": "var(--border)" } as const;
  const primaryBtn =
    "px-4 py-2 rounded-lg text-sm font-medium transition-colors hover:opacity-90 disabled:opacity-40";
  const ghostBtn = "px-4 py-2 rounded-lg text-sm font-medium border transition-colors hover:opacity-80";

  return (
    <div class="h-screen overflow-y-auto" style={{ "background-color": "var(--bg)", color: "var(--text-primary)" }}>
      {/* Header bar (draggable) */}
      <div
        class="sticky top-0 z-10 flex items-center h-12 px-5 border-b"
        style={{
          "background-color": "var(--surface)",
          "border-color": "var(--border)",
          "-webkit-app-region": "drag",
        } as any}
      >
        <div class="flex items-center gap-2">
          <span class="text-lg">🎙️</span>
          <span class="text-base font-semibold tracking-tight">Welcome to Canario</span>
        </div>
        <div class="ml-auto" style={{ "-webkit-app-region": "no-drag" } as any}>
          <button
            class="text-xs px-2 py-1 rounded-md hover:opacity-80 transition-opacity"
            style={{ color: "var(--text-secondary)", cursor: "pointer" }}
            onClick={() => completeWizard(false)}
          >
            Skip setup
          </button>
        </div>
      </div>

      <div class="max-w-lg mx-auto px-5 py-8 flex flex-col gap-5 animate-window-appear">
        {/* Tagline */}
        <div class="text-center">
          <p class="text-sm" style={{ color: "var(--text-secondary)" }}>
            Voice-to-text, instant and invisible. Press a hotkey, speak, release. Done.
          </p>
        </div>

        {/* Step indicator */}
        <div class="flex items-center gap-2">
          <For each={STEP_LABELS}>
            {(label, i) => (
              <div class="flex-1 flex flex-col gap-1.5">
                <div
                  class="h-1 rounded-full transition-colors"
                  style={{
                    "background-color": step() >= i() + 1 ? "var(--accent)" : "var(--border)",
                  }}
                />
                <span
                  class="text-[11px] text-center"
                  style={{ color: step() === i() + 1 ? "var(--text-primary)" : "var(--text-secondary)" }}
                >
                  {label}
                </span>
              </div>
            )}
          </For>
        </div>

        {/* ── Step 1: Download Model ─────────────────────────────── */}
        <Show when={step() === 1}>
          <section class="rounded-xl border p-5 flex flex-col gap-4" style={sectionStyle}>
            <div>
              <h2 class="text-base font-semibold">Step 1 of 3: Download Model</h2>
              <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                Canario uses Parakeet TDT — a state-of-the-art speech recognition model that runs
                entirely on your device. Nothing you say ever leaves your machine.
              </p>
            </div>

            <div class="flex flex-col gap-2">
              <For each={MODELS}>
                {(model) => (
                  <button
                    class="flex items-center justify-between p-3 rounded-lg border transition-colors cursor-pointer"
                    style={{
                      "background-color": selectedModel() === model.id ? "var(--surface-hover)" : "transparent",
                      "border-color": selectedModel() === model.id ? "var(--accent)" : "var(--border)",
                    }}
                    onClick={() => handleSelectModel(model.id)}
                  >
                    <div class="text-left">
                      <p class="text-sm font-medium">{model.name}</p>
                      <p class="text-xs mt-0.5" style={{ color: "var(--text-secondary)" }}>{model.desc}</p>
                    </div>
                    <div class="w-4 h-4 rounded-full border-2 flex items-center justify-center"
                      style={{ "border-color": selectedModel() === model.id ? "var(--accent)" : "var(--border)" }}
                    >
                      <Show when={selectedModel() === model.id}>
                        <div class="w-2 h-2 rounded-full" style={{ "background-color": "var(--accent)" }} />
                      </Show>
                    </div>
                  </button>
                )}
              </For>
            </div>

            <Show
              when={!modelReady()}
              fallback={
                <p class="text-sm font-medium text-center py-1" style={{ color: "var(--success)" }}>
                  ✓ {MODELS.find((m) => m.id === selectedModel())?.name} is ready
                </p>
              }
            >
              <Show
                when={dlProgress() !== null}
                fallback={
                  <button
                    class="w-full py-2.5 rounded-lg text-sm font-medium transition-colors hover:opacity-90"
                    style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                    onClick={handleDownload}
                  >
                    Download {MODELS.find((m) => m.id === selectedModel())?.name}
                  </button>
                }
              >
                <div class="flex flex-col gap-1.5">
                  <div class="flex items-center gap-3">
                    <div class="flex-1 h-2 rounded-full overflow-hidden" style={{ "background-color": "var(--border)" }}>
                      <div
                        class="h-full rounded-full transition-all duration-300"
                        style={{ width: `${(dlProgress() ?? 0) * 100}%`, "background-color": "var(--accent)" }}
                      />
                    </div>
                    <span class="text-sm tabular-nums w-12 text-right" style={{ color: "var(--text-secondary)" }}>
                      {Math.round((dlProgress() ?? 0) * 100)}%
                    </span>
                  </div>
                  <p class="text-xs text-center tabular-nums" style={{ color: "var(--text-secondary)" }}>
                    {dlStats()}
                  </p>
                </div>
              </Show>
            </Show>

            {/* Mic test widget */}
            <div class="rounded-lg border p-3 flex flex-col gap-2" style={{ "border-color": "var(--border)", "background-color": "var(--bg)" }}>
              <div class="flex items-center justify-between">
                <p class="text-sm font-medium">🎤 Microphone Test</p>
                <button
                  class="text-xs px-2.5 py-1 rounded-md border transition-colors hover:opacity-80 disabled:opacity-40"
                  style={{ "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                  onClick={() => (micTesting() ? stopMicTest() : startMicTest())}
                >
                  {micTesting() ? "Stop" : "Test microphone"}
                </button>
              </div>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                {micTesting() ? "Say something..." : `Records ${MIC_TEST_MS / 1000}s of audio to check your mic level.`}
              </p>
              <div class="h-2 rounded-full overflow-hidden" style={{ "background-color": "var(--border)" }}>
                <div
                  class="h-full rounded-full"
                  style={{
                    width: `${Math.min(1, level() * 3) * 100}%`,
                    "background-color": micTesting() ? "var(--success)" : "var(--border)",
                    transition: "width 60ms linear",
                  }}
                />
              </div>
            </div>

            <div class="flex justify-end">
              <button
                class={primaryBtn}
                style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                onClick={() => gotoStep(2)}
              >
                Next →
              </button>
            </div>
            <Show when={!modelReady() && dlProgress() === null}>
              <p class="text-xs text-center -mt-2" style={{ color: "var(--text-secondary)" }}>
                You can continue without the model, but transcription won't work until it's downloaded.
              </p>
            </Show>
          </section>
        </Show>

        {/* ── Step 2: Set Hotkey ─────────────────────────────────── */}
        <Show when={step() === 2}>
          <section class="rounded-xl border p-5 flex flex-col gap-4" style={sectionStyle}>
            <div>
              <h2 class="text-base font-semibold">Step 2 of 3: Set Hotkey</h2>
              <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                Pick a key combination that starts and stops recording from anywhere.
              </p>
            </div>

            <HotkeyCapture value={hotkey()} onChange={handleHotkeyChange} />

            <div class="rounded-lg p-3 flex flex-col gap-1.5" style={{ "background-color": "var(--bg)" }}>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                <strong style={{ color: "var(--text-primary)" }}>Press-and-hold:</strong> hold the combo
                while you speak, release to transcribe.
              </p>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                <strong style={{ color: "var(--text-primary)" }}>Double-tap:</strong> tap the combo to start
                recording, tap again to stop — hands-free for longer dictation.
              </p>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                {platform().isLinux
                  ? "On Linux the hotkey is handled by Canario's own listener."
                  : "On this platform the hotkey is registered globally with the OS."}
              </p>
            </div>

            <div class="flex justify-between">
              <button
                class={ghostBtn}
                style={{ "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                onClick={() => gotoStep(1)}
              >
                ← Back
              </button>
              <button
                class={primaryBtn}
                style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                onClick={() => gotoStep(3)}
              >
                Next →
              </button>
            </div>
          </section>
        </Show>

        {/* ── Step 3: Ready ──────────────────────────────────────── */}
        <Show when={step() === 3}>
          <section class="rounded-xl border p-5 flex flex-col gap-4" style={sectionStyle}>
            <div>
              <h2 class="text-base font-semibold">Step 3 of 3: Ready</h2>
              <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                Try it now! Click the field below, press{" "}
                <strong style={{ color: "var(--text-primary)" }}>
                  {hotkey().length > 0 ? hotkey().join(" + ") : "your hotkey"}
                </strong>
                , speak, and release — your words will appear right here.
              </p>
            </div>

            <textarea
              ref={practiceRef}
              rows={4}
              placeholder="Press your hotkey and say something…"
              class="rounded-lg border text-sm w-full p-3 resize-none"
              style={{
                "background-color": "var(--bg)",
                "border-color": "var(--border)",
                color: "var(--text-primary)",
                outline: "none",
              }}
            />

            <Show when={context().lastTranscription}>
              <p class="text-xs" style={{ color: "var(--success)" }}>
                ✓ It works! Last transcription: "{context().lastTranscription}"
              </p>
            </Show>
            <Show when={!modelReady()}>
              <p class="text-xs" style={{ color: "var(--warning)" }}>
                Heads up: no speech model is downloaded yet, so practice dictation won't transcribe.
                You can download it later from Settings → Model.
              </p>
            </Show>

            <div class="flex items-center justify-between">
              <div>
                <p class="text-sm font-medium">Start on login</p>
                <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Launch Canario when you log in</p>
              </div>
              <Toggle checked={autostart()} onChange={handleAutostart} />
            </div>

            <div class="flex justify-between">
              <button
                class={ghostBtn}
                style={{ "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                onClick={() => gotoStep(2)}
              >
                ← Back
              </button>
              <button
                class={primaryBtn}
                style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                onClick={() => completeWizard(true)}
              >
                Done — minimize to tray
              </button>
            </div>
          </section>
        </Show>
      </div>

      <ToastContainer />
    </div>
  );
}

// Onboarding wizard — PRD §5.1
// 3 steps: Download Model → Set Hotkey → Ready (practice area)
// Reached when the state machine is in `onboarding` (see App.tsx routing).
import { createSignal, Show, For, onMount, onCleanup, createEffect } from "solid-js";
import { t, type MessageKey } from "../i18n";
import { useAppState } from "../state/context";
import { createCanario } from "../primitives/createCanario";
import { HotkeyCapture, toAccelerator } from "../components/HotkeyCapture";
import { Toggle } from "../components/Toggle";
import { ToastContainer, showToast } from "../components/Toast";
import { applyTheme } from "../theme";

// Display name/description live in the i18n catalog (canario-7ah.7);
// ids are the AppConfig model identifiers and stay literal.
const MODELS = [
  { id: "ParakeetV3", nameKey: "model.parakeetV3.name", descKey: "model.parakeetV3.desc" },
  { id: "ParakeetV2", nameKey: "model.parakeetV2.name", descKey: "model.parakeetV2.desc" },
] as const satisfies ReadonlyArray<{ id: string; nameKey: MessageKey; descKey: MessageKey }>;

const MODEL_SIZE_MB = 640;
const STEP_LABEL_KEYS = ["onboarding.step.downloadModel", "onboarding.step.setHotkey", "onboarding.step.ready"] as const satisfies readonly MessageKey[];
const MIC_TEST_MS = 3000;

function formatEta(seconds: number): string {
  if (!isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return t("common.etaSeconds", { n: Math.ceil(seconds) });
  return t("common.etaMinutes", { m: Math.floor(seconds / 60), s: Math.ceil(seconds % 60) });
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
    const base = t("onboarding.dlStats.plain", { done, total: MODEL_SIZE_MB });
    if (speedEma > 0.1) {
      const eta = formatEta(((1 - p) * MODEL_SIZE_MB) / speedEma);
      return (
        t("onboarding.dlStats.speed", { done, total: MODEL_SIZE_MB, speed: speedEma.toFixed(1) }) +
        (eta ? t("onboarding.dlStats.etaSuffix", { eta }) : "")
      );
    }
    return base;
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
      showToast(t("onboarding.micTest.noAccess"), "error", 6000);
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
      showToast(t("behavior.autostart.failed"), "warning");
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
      showToast(t("onboarding.initFailed"), "error", 8000);
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
            showToast(t("onboarding.step1.modelDownloaded"), "success", 3000);
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
          <span class="text-base font-semibold tracking-tight">{t("onboarding.header")}</span>
        </div>
        <div class="ml-auto" style={{ "-webkit-app-region": "no-drag" } as any}>
          <button
            class="text-xs px-2 py-1 rounded-md hover:opacity-80 transition-opacity"
            style={{ color: "var(--text-secondary)", cursor: "pointer" }}
            onClick={() => completeWizard(false)}
          >
            {t("onboarding.skip")}
          </button>
        </div>
      </div>

      <div class="max-w-lg mx-auto px-5 py-8 flex flex-col gap-5 animate-window-appear">
        {/* Tagline */}
        <div class="text-center">
          <p class="text-sm" style={{ color: "var(--text-secondary)" }}>
            {t("onboarding.tagline")}
          </p>
        </div>

        {/* Step indicator */}
        <div class="flex items-center gap-2">
          <For each={STEP_LABEL_KEYS}>
            {(labelKey, i) => (
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
                  {t(labelKey)}
                </span>
              </div>
            )}
          </For>
        </div>

        {/* ── Step 1: Download Model ─────────────────────────────── */}
        <Show when={step() === 1}>
          <section class="rounded-xl border p-5 flex flex-col gap-4" style={sectionStyle}>
            <div>
              <h2 class="text-base font-semibold">{t("onboarding.step1.title")}</h2>
              <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                {t("onboarding.step1.desc")}
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
                      <p class="text-sm font-medium">{t(model.nameKey)}</p>
                      <p class="text-xs mt-0.5" style={{ color: "var(--text-secondary)" }}>{t(model.descKey)}</p>
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
                  {t("model.ready", { name: t(MODELS.find((m) => m.id === selectedModel())?.nameKey ?? "model.parakeetV3.name") })}
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
                    {t("model.download", { name: t(MODELS.find((m) => m.id === selectedModel())?.nameKey ?? "model.parakeetV3.name") })}
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
                    <button
                      class="px-2.5 py-1 rounded-md text-xs border transition-colors hover:opacity-80 shrink-0"
                      style={{
                        "background-color": "var(--bg)",
                        "border-color": "var(--border)",
                        color: "var(--text-secondary)",
                        cursor: "pointer",
                      }}
                      title={t("model.stopDownloadTitle")}
                      onClick={() => {
                        void window.canario?.sendCommand({ id: `cancel-dl-${Date.now()}`, cmd: "cancel_download" });
                        showToast(t("model.downloadCancelled"), "info", 3000);
                      }}
                    >
                      {t("common.cancel")}
                    </button>
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
                <p class="text-sm font-medium">{t("onboarding.micTest.title")}</p>
                <button
                  class="text-xs px-2.5 py-1 rounded-md border transition-colors hover:opacity-80 disabled:opacity-40"
                  style={{ "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                  onClick={() => (micTesting() ? stopMicTest() : startMicTest())}
                >
                  {micTesting() ? t("onboarding.micTest.stop") : t("onboarding.micTest.start")}
                </button>
              </div>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                {micTesting()
                  ? t("onboarding.micTest.saying")
                  : t("onboarding.micTest.desc", { secs: MIC_TEST_MS / 1000 })}
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
                {t("onboarding.next")}
              </button>
            </div>
            <Show when={!modelReady() && dlProgress() === null}>
              <p class="text-xs text-center -mt-2" style={{ color: "var(--text-secondary)" }}>
                {t("onboarding.step1.continueWithout")}
              </p>
            </Show>
          </section>
        </Show>

        {/* ── Step 2: Set Hotkey ─────────────────────────────────── */}
        <Show when={step() === 2}>
          <section class="rounded-xl border p-5 flex flex-col gap-4" style={sectionStyle}>
            <div>
              <h2 class="text-base font-semibold">{t("onboarding.step2.title")}</h2>
              <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                {t("onboarding.step2.desc")}
              </p>
            </div>

            <HotkeyCapture value={hotkey()} onChange={handleHotkeyChange} />

            <div class="rounded-lg p-3 flex flex-col gap-1.5" style={{ "background-color": "var(--bg)" }}>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                <strong style={{ color: "var(--text-primary)" }}>{t("onboarding.step2.pressHoldLabel")}</strong>{" "}
                {t("onboarding.step2.pressHoldBody")}
              </p>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                <strong style={{ color: "var(--text-primary)" }}>{t("onboarding.step2.doubleTapLabel")}</strong>{" "}
                {t("onboarding.step2.doubleTapBody")}
              </p>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                {platform().isLinux
                  ? t("onboarding.step2.linux")
                  : t("onboarding.step2.other")}
              </p>
            </div>

            <div class="flex justify-between">
              <button
                class={ghostBtn}
                style={{ "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                onClick={() => gotoStep(1)}
              >
                {t("onboarding.back")}
              </button>
              <button
                class={primaryBtn}
                style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                onClick={() => gotoStep(3)}
              >
                {t("onboarding.next")}
              </button>
            </div>
          </section>
        </Show>

        {/* ── Step 3: Ready ──────────────────────────────────────── */}
        <Show when={step() === 3}>
          <section class="rounded-xl border p-5 flex flex-col gap-4" style={sectionStyle}>
            <div>
              <h2 class="text-base font-semibold">{t("onboarding.step3.title")}</h2>
              <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                {t("onboarding.step3.descIntro")}{" "}
                <strong style={{ color: "var(--text-primary)" }}>
                  {hotkey().length > 0 ? hotkey().join(" + ") : t("onboarding.step3.yourHotkey")}
                </strong>
                {t("onboarding.step3.descOutro")}
              </p>
            </div>

            <textarea
              ref={practiceRef}
              rows={4}
              placeholder={t("onboarding.step3.placeholder")}
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
                {t("onboarding.step3.works", { text: context().lastTranscription ?? "" })}
              </p>
            </Show>
            <Show when={!modelReady()}>
              <p class="text-xs" style={{ color: "var(--warning)" }}>
                {t("onboarding.step3.noModel")}
              </p>
            </Show>

            <div class="flex items-center justify-between">
              <div>
                <p class="text-sm font-medium">{t("behavior.autostart.title")}</p>
                <p class="text-xs" style={{ color: "var(--text-secondary)" }}>{t("behavior.autostart.desc")}</p>
              </div>
              <Toggle checked={autostart()} onChange={handleAutostart} />
            </div>

            <div class="flex justify-between">
              <button
                class={ghostBtn}
                style={{ "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                onClick={() => gotoStep(2)}
              >
                {t("onboarding.back")}
              </button>
              <button
                class={primaryBtn}
                style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                onClick={() => completeWizard(true)}
              >
                {t("onboarding.done")}
              </button>
            </div>
          </section>
        </Show>
      </div>

      <ToastContainer />
    </div>
  );
}

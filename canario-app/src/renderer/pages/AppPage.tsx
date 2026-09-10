// Settings / Main page for Canario Electron
// Phase 2: Polish - animations, error states, empty states, autostart
import { createSignal, Show, For, onMount, onCleanup, createEffect } from "solid-js";
import { createVirtualizer } from "@tanstack/solid-virtual";
import { useAppState } from "../state/context";
import { createCanario } from "../primitives/createCanario";
import { HotkeyCapture, toAccelerator } from "../components/HotkeyCapture";
import { WordRemapping } from "../components/WordRemapping";
import { Toggle } from "../components/Toggle";
import { ToastContainer, showToast } from "../components/Toast";
import { applyTheme } from "../theme";
import {
  CUSTOM_MODEL_ID,
  customModelStatus,
  customPathConfigKey,
  pickFileFilters,
  type CustomModelPaths,
} from "../primitives/customModel";

const MODELS = [
  { id: "ParakeetV3", name: "Parakeet TDT v3", desc: "Multilingual · ~640MB" },
  { id: "ParakeetV2", name: "Parakeet TDT v2", desc: "English only · ~640MB" },
] as const;

// Local-only variant — no download; core resolves custom_*_path files.
const CUSTOM_MODEL = { id: CUSTOM_MODEL_ID, name: "Custom model", desc: "Local sherpa-onnx files · no download" };
const ALL_MODELS = [...MODELS, CUSTOM_MODEL];

type HistoryEntry = { id: string; text: string; duration_secs: number; timestamp: string };

export function AppPage() {
  const machine = useAppState();
  const canario = createCanario(machine);
  const { state, context, updateContext, send } = machine;

  const [history, setHistory] = createSignal<HistoryEntry[]>([]);
  const [selectedModel, setSelectedModel] = createSignal("ParakeetV3");
  const [downloadedModels, setDownloadedModels] = createSignal<Set<string>>(new Set());
  const [loading, setLoading] = createSignal(true);
  const [historySearch, setHistorySearch] = createSignal("");
  const [theme, setTheme] = createSignal<string>("dark");

  // Track history items being animated out
  const [deletingIds, setDeletingIds] = createSignal<Set<string>>(new Set());

  // Config state
  const [hotkey, setHotkey] = createSignal<string[]>([]);
  const [autoPaste, setAutoPaste] = createSignal(true);
  const [soundEffects, setSoundEffects] = createSignal(true);
  const [autostart, setAutostart] = createSignal(false);
  const [showTrayIcon, setShowTrayIcon] = createSignal(true);
  const [audioBehavior, setAudioBehavior] = createSignal<string>("DoNothing");
  const [customPaths, setCustomPaths] = createSignal<CustomModelPaths>({ encoder: "", decoder: "", tokens: "" });
  const [remappings, setRemappings] = createSignal<{ from: string; to: string }[]>([]);
  const [removals, setRemovals] = createSignal<{ word: string }[]>([]);
  const [platform, setPlatform] = createSignal<{ isLinux: boolean; isMac: boolean; isWindows: boolean }>({ isLinux: true, isMac: false, isWindows: false });

  // Track whether the initial model check is done
  const [initialModelCheck, setInitialModelCheck] = createSignal(false);

  // About / Updates
  const [appVersion, setAppVersion] = createSignal("...");
  const [updateChecking, setUpdateChecking] = createSignal(false);
  const [updateAvailable, setUpdateAvailable] = createSignal(false);
  const [updateVersion, setUpdateVersion] = createSignal("");

  async function handleCheckUpdate() {
    setUpdateChecking(true);
    try {
      const result = await canario.checkForUpdate();
      if (result?.available) {
        setUpdateAvailable(true);
        setUpdateVersion(result.version || "?");
      } else {
        showToast("Canario is up to date", "success", 3000);
      }
    } catch {
      showToast("Could not check for updates", "error");
    }
    setUpdateChecking(false);
  }

  async function checkModelDownloaded(modelId: string): Promise<boolean> {
    const res = await canario.command("is_model_downloaded", { model: modelId });
    return !!(res?.ok) && res.data === true;
  }

  // Apply theme
  createEffect(() => {
    applyTheme(theme());
  });

  // Clear error after it's been shown
  createEffect(() => {
    const err = context().lastError;
    if (err) {
      showToast(err, "error", 6000);
      // Clear error so it doesn't re-show
      updateContext({ lastError: null });
    }
  });

  async function loadHistory() {
    const query = historySearch().trim();
    let res: Record<string, unknown> | null;
    if (query) {
      res = await canario.searchHistory(query);
    } else {
      res = await canario.getHistory(1000);
    }
    if (res?.ok && Array.isArray(res.data)) {
      setHistory(res.data as HistoryEntry[]);
    }
  }

  onMount(async () => {
    try {
      // Platform info
      const p = await canario.getPlatform();
      if (p) setPlatform(p);

      // Theme
      const savedTheme = await canario.getTheme();
      setTheme(savedTheme || "dark");

      // 1. Get current config
      const cfg = await canario.getConfig();
      if (cfg) {
        const config = cfg as Record<string, unknown>;
        const modelId = (config.model as string) || "ParakeetV3";
        setSelectedModel(modelId);
        setHotkey((config.hotkey as string[]) || []);
        setAutoPaste((config.auto_paste as boolean) ?? true);
        setSoundEffects((config.sound_effects as boolean) ?? true);
        setAutostart((config.autostart as boolean) ?? false);
        setShowTrayIcon((config.show_tray_icon as boolean) ?? true);
        setAudioBehavior((config.recording_audio_behavior as string) || "DoNothing");
        setCustomPaths({
          encoder: (config.custom_encoder_path as string) || "",
          decoder: (config.custom_decoder_path as string) || "",
          tokens: (config.custom_tokens_path as string) || "",
        });

        // Post-processor
        const pp = config.post_processor as Record<string, unknown> | undefined;
        if (pp) {
          setRemappings((pp.remappings as { from: string; to: string }[]) || []);
          setRemovals((pp.removals as { word: string }[]) || []);
        }

        updateContext({ config });
      }

      // 2. Check which models are downloaded
      for (const m of MODELS) {
        const ready = await checkModelDownloaded(m.id);
        if (ready) {
          setDownloadedModels(prev => new Set([...prev, m.id]));
        }
      }

      // Inventory checks leave the selected model unchanged.
      const originalModel = (cfg as Record<string, unknown>)?.model as string || "ParakeetV3";
      setSelectedModel(originalModel);

      // 3. Update app state — Custom is local-only, so ask the core whether
      // the resolved custom_*_path files exist instead of the download set.
      let currentReady: boolean;
      if (originalModel === CUSTOM_MODEL_ID) {
        const res = await canario.command("is_model_downloaded");
        currentReady = !!(res?.ok) && res.data === true;
      } else {
        currentReady = downloadedModels().has(originalModel);
      }
      updateContext({ modelReady: currentReady });
      setInitialModelCheck(true);

      // 4. Auto-load history
      await loadHistory();

      // 5. Get version info
      const ver = await canario.getVersion();
      if (ver) setAppVersion(ver.electron + (ver.sidecar ? ` (sidecar ${ver.sidecar})` : ""));
    } catch (err) {
      console.error("[AppPage] init error:", err);
      showToast("Failed to initialize. Check that canario-electron sidecar is running.", "error", 8000);
    }
    setLoading(false);
  });

  // Tray "History" menu item — scroll the History section into view
  onMount(() => {
    const unsub = canario.onNavigateHistory(() => {
      requestAnimationFrame(() => {
        historySectionRef?.scrollIntoView({ behavior: "smooth", block: "start" });
        historySectionRef?.focus({ preventScroll: true });
      });
    });
    onCleanup(unsub);
  });

  // A download finished (possibly for a variant other than the one now
  // selected): refresh the per-variant readiness set so the Model section
  // flips from "Download" to "Ready" without a remount, and re-derive
  // readiness for the current selection.
  onMount(() => {
    const unsub = canario.onModelDownloadComplete(() => {
      void (async () => {
        const modelId = selectedModel();
        if (modelId === CUSTOM_MODEL_ID) {
          await canario.checkModel();
          return;
        }
        const ready = await checkModelDownloaded(modelId);
        if (ready) {
          setDownloadedModels((prev) => new Set([...prev, modelId]));
        }
        updateContext({ modelReady: ready });
      })();
    });
    onCleanup(unsub);
  });

  async function handleToggle() {
    const res = await canario.toggleRecording();
    if (res && !res.ok) {
      const errMsg = (res.error as string) || "Recording failed";
      if (errMsg.toLowerCase().includes("no mic") || errMsg.toLowerCase().includes("microphone") || errMsg.toLowerCase().includes("audio")) {
        showToast("No microphone detected. Check your audio settings.", "error", 6000);
      } else {
        showToast(errMsg, "error");
      }
    }
  }

  async function handleSelectModel(modelId: string) {
    setSelectedModel(modelId);
    await canario.updateConfig({ model: modelId });
    if (modelId === CUSTOM_MODEL_ID) {
      // Local-only: readiness = the core finds all resolved custom files
      await canario.checkModel();
    } else {
      const ready = downloadedModels().has(modelId);
      updateContext({ modelReady: ready });
    }
  }

  // Custom model: pick one of the three local sherpa-onnx files
  async function handlePickCustomPath(field: keyof CustomModelPaths) {
    const path = await canario.pickFile(pickFileFilters(field));
    if (!path) return; // dialog cancelled
    setCustomPaths((prev) => ({ ...prev, [field]: path }));
    await canario.updateConfig({ [customPathConfigKey(field)]: path });
    // Refresh readiness — the core verifies the resolved files exist
    await canario.checkModel();
  }

  async function handleDownload() {
    await canario.downloadModel();
  }

  async function handleHotkeyChange(keys: string[]) {
    setHotkey(keys);
    await canario.updateConfig({ hotkey: keys });

    // Restart hotkey listener with new config
    if (platform().isLinux) {
      await canario.restartHotkey();
    } else {
      // macOS/Windows: register via Electron API
      if (keys.length > 0) {
        await canario.registerShortcut(toAccelerator(keys));
      }
    }
  }

  async function handleConfigToggle(field: string, value: boolean) {
    await canario.updateConfig({ [field]: value });
    if (field === "auto_paste") setAutoPaste(value);
    if (field === "sound_effects") setSoundEffects(value);
    if (field === "show_tray_icon") {
      setShowTrayIcon(value);
      if (!value) {
        showToast("Tray icon hidden — relaunch Canario to reopen this window", "info", 5000);
      }
    }
    if (field === "autostart") {
      setAutostart(value);
      const ok = await canario.setAutostart(value);
      if (!ok) {
        showToast("Could not change autostart setting.", "warning");
        setAutostart(!value);
        return;
      }
      showToast(value ? "Canario will start on login" : "Autostart disabled", "info", 3000);
    }
  }

  async function handleAudioBehaviorChange(behavior: string) {
    setAudioBehavior(behavior);
    await canario.updateConfig({ recording_audio_behavior: behavior });
  }

  async function handlePostProcessorChange(pp: { remappings: { from: string; to: string }[]; removals: { word: string }[] }) {
    setRemappings(pp.remappings);
    setRemovals(pp.removals);
    await canario.updateConfig({
      post_processor: {
        remappings: pp.remappings,
        removals: pp.removals,
      },
    });
  }

  async function handleDeleteHistory(id: string) {
    // Animate out first
    setDeletingIds(prev => new Set([...prev, id]));
    await new Promise(r => setTimeout(r, 200));
    await canario.deleteHistory(id);
    await loadHistory();
    setDeletingIds(prev => {
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
  }

  async function handleClearHistory() {
    await canario.clearHistory();
    setHistory([]);
    showToast("History cleared", "info", 2000);
  }

  async function handleThemeChange(t: string) {
    setTheme(t);
    await canario.setTheme(t);
  }

  // Re-run the onboarding wizard (PRD §5.1): clear the persisted flag and
  // move the state machine back into `onboarding` — App.tsx routes on it.
  async function handleRerunOnboarding() {
    await canario.setOnboardingCompleted(false);
    send({ type: "START_ONBOARDING" });
  }

  const currentModelDownloaded = () => downloadedModels().has(selectedModel());
  const currentModel = () => ALL_MODELS.find(m => m.id === selectedModel()) || MODELS[0];

  // Custom model status — derived from the configured paths + the core's
  // is_model_downloaded verdict (context().modelReady while Custom is selected)
  const customStatus = () => customModelStatus(customPaths(), context().modelReady);
  const customMissing = () => {
    const s = customStatus();
    return s.kind === "missing-paths" ? s.missing : null;
  };

  // ── Virtualized history list ────────────────────────────────────
  // The list lives inside the page-level scroll container (below several
  // conditional sections), so the virtualizer tracks that scroller plus a
  // scrollMargin equal to the list's offset within it.
  const [scrollEl, setScrollEl] = createSignal<HTMLDivElement | null>(null);
  const [historyListEl, setHistoryListEl] = createSignal<HTMLDivElement | null>(null);
  const [historyListOffset, setHistoryListOffset] = createSignal(0);
  let historySectionRef: HTMLElement | undefined;

  const historyVirtualizer = createVirtualizer<HTMLDivElement, HTMLDivElement>({
    get count() {
      return history().length;
    },
    getScrollElement: () => scrollEl(),
    estimateSize: () => 86,
    overscan: 6,
    gap: 8,
    get scrollMargin() {
      return historyListOffset();
    },
  });

  // Recompute the list offset when any layout above it can shift.
  createEffect(() => {
    history();
    loading();
    context().lastTranscription;
    context().modelReady;
    initialModelCheck();
    downloadedModels();
    state().status;
    const list = historyListEl();
    const scroller = scrollEl();
    if (!list || !scroller) return;
    setHistoryListOffset(
      list.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop
    );
  });

  // Reusable styles
  const sectionStyle = { "background-color": "var(--surface)", "border-color": "var(--border)" } as const;
  const sectionHeader = "text-sm font-semibold uppercase tracking-wider mb-4";
  const sectionHeaderStyle = { color: "var(--text-secondary)" } as const;

  return (
    <div ref={setScrollEl} class="h-screen overflow-y-auto" style={{ "background-color": "var(--bg)", color: "var(--text-primary)" }}>
      {/* Header bar */}
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
          <span class="text-base font-semibold tracking-tight">Canario</span>
        </div>
        <div class="ml-auto flex items-center gap-3" style={{ "-webkit-app-region": "no-drag" } as any}>
          <Show when={state().status === "recording"}>
            <div class="flex items-center gap-1.5">
              <div class="w-2.5 h-2.5 rounded-full animate-pulse-dot" style={{ "background-color": "var(--recording-dot)" }} />
              <span class="text-sm font-medium" style={{ color: "var(--recording-dot)" }}>REC</span>
            </div>
          </Show>
          <Show when={state().status === "idle" && context().modelReady}>
            <button
              class="px-4 py-1.5 rounded-lg text-sm font-medium transition-colors hover:opacity-90"
              style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
              onClick={handleToggle}
            >
              🎤 Record
            </button>
          </Show>
        </div>
      </div>

      <Show when={!loading()} fallback={
        <div class="flex items-center justify-center h-64">
          <div class="flex flex-col items-center gap-3">
            <div class="w-6 h-6 rounded-full border-2 border-t-transparent animate-spin" style={{ "border-color": "var(--accent)", "border-top-color": "transparent" }} />
            <p class="text-sm" style={{ color: "var(--text-secondary)" }}>Loading...</p>
          </div>
        </div>
      }>
        <div class="max-w-lg mx-auto px-5 py-6 flex flex-col gap-5 animate-window-appear">

          {/* ── Model section ─────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Model</h2>

            <div class="flex flex-col gap-2 mb-4">
              <For each={ALL_MODELS}>
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
                    <div class="flex items-center gap-2">
                      <Show when={downloadedModels().has(model.id)}>
                        <span class="text-xs" style={{ color: "var(--success)" }}>✓</span>
                      </Show>
                      <div class="w-4 h-4 rounded-full border-2 flex items-center justify-center"
                        style={{ "border-color": selectedModel() === model.id ? "var(--accent)" : "var(--border)" }}
                      >
                        <Show when={selectedModel() === model.id}>
                          <div class="w-2 h-2 rounded-full" style={{ "background-color": "var(--accent)" }} />
                        </Show>
                      </div>
                    </div>
                  </button>
                )}
              </For>
            </div>

            <Show
              when={selectedModel() !== CUSTOM_MODEL_ID}
              fallback={
                /* Custom model: local-only — no Download button (the core
                   rejects download_model for Custom). Configure the three
                   sherpa-onnx paths instead. */
                <div class="flex flex-col gap-3">
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                    Point Canario at your own sherpa-onnx files. joiner.int8.onnx must sit next to the encoder.
                  </p>
                  <For each={(["encoder", "decoder", "tokens"] as const)}>
                    {(field) => (
                      <div class="flex items-center gap-2">
                        <span class="text-xs w-16 shrink-0 capitalize" style={{ color: "var(--text-secondary)" }}>
                          {field}
                        </span>
                        <input
                          type="text"
                          readOnly
                          value={customPaths()[field]}
                          placeholder="Not set"
                          title={customPaths()[field]}
                          class="flex-1 min-w-0 px-2 py-1.5 rounded-lg border text-xs"
                          style={{
                            "background-color": "var(--bg)",
                            "border-color": "var(--border)",
                            color: "var(--text-primary)",
                            outline: "none",
                          }}
                        />
                        <button
                          class="px-2.5 py-1.5 rounded-lg text-xs font-medium border transition-colors hover:opacity-80 shrink-0"
                          style={{
                            "background-color": "var(--bg)",
                            "border-color": "var(--border)",
                            color: "var(--text-primary)",
                            cursor: "pointer",
                          }}
                          onClick={() => handlePickCustomPath(field)}
                        >
                          Browse…
                        </button>
                      </div>
                    )}
                  </For>
                  <Show when={customMissing()}>
                    <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                      ⚠ Set the {customMissing()!.join(", ")} path{customMissing()!.length > 1 ? "s" : ""} — recording will fail until all three are configured.
                    </p>
                  </Show>
                  <Show when={customStatus().kind === "files-missing"}>
                    <p class="text-xs" style={{ color: "var(--error)" }}>
                      ⚠ One or more model files weren't found on disk — check the paths above.
                    </p>
                  </Show>
                  <Show when={customStatus().kind === "ready"}>
                    <p class="text-sm font-medium py-1" style={{ color: "var(--success)" }}>
                      ✓ Custom model is ready
                    </p>
                  </Show>
                </div>
              }
            >
            <Show
              when={currentModelDownloaded()}
              fallback={
                <Show
                  when={state().status === "downloading"}
                  fallback={
                    <div class="flex flex-col gap-3">
                      <button
                        class="w-full py-2.5 rounded-lg text-sm font-medium transition-colors hover:opacity-90"
                        style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                        onClick={handleDownload}
                      >
                        Download {currentModel().name}
                      </button>
                      <Show when={initialModelCheck() && !currentModelDownloaded()}>
                        <p class="text-xs text-center" style={{ color: "var(--text-secondary)" }}>
                          The ASR model runs locally on your device. Download required before first use.
                        </p>
                      </Show>
                    </div>
                  }
                >
                  <div class="flex items-center gap-3">
                    <div class="flex-1 h-2 rounded-full overflow-hidden" style={{ "background-color": "var(--border)" }}>
                      <div
                        class="h-full rounded-full transition-all duration-300"
                        style={{
                          width: `${((state() as any).progress || 0) * 100}%`,
                          "background-color": "var(--accent)",
                        }}
                      />
                    </div>
                    <span class="text-sm tabular-nums w-12 text-right" style={{ color: "var(--text-secondary)" }}>
                      {(((state() as any).progress || 0) * 100).toFixed(0)}%
                    </span>
                  </div>
                  <p class="text-xs mt-2 text-center" style={{ color: "var(--text-secondary)" }}>
                    Downloading model... this may take a few minutes.
                  </p>
                </Show>
              }
            >
              <div class="flex items-center justify-between py-1">
                <span class="text-sm font-medium" style={{ color: "var(--success)" }}>
                  ✓ {currentModel().name} is ready
                </span>
                <button
                  class="text-xs px-2 py-1 rounded-md hover:opacity-80 transition-opacity"
                  style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                  onClick={async () => {
                    await canario.deleteModel();
                    setDownloadedModels(prev => {
                      const next = new Set(prev);
                      next.delete(selectedModel());
                      return next;
                    });
                    updateContext({ modelReady: false });
                    showToast("Model deleted", "info", 3000);
                  }}
                >
                  Delete
                </button>
              </div>
            </Show>
            </Show>
          </section>

          {/* ── Quick Record ──────────────────────────────────────── */}
          <Show when={context().modelReady}>
            <section class="rounded-xl border p-5" style={sectionStyle}>
              <h2 class={sectionHeader} style={sectionHeaderStyle}>Record</h2>
              <div class="flex items-center justify-center py-3">
                <Show
                  when={state().status !== "recording"}
                  fallback={
                    <button
                      class="w-16 h-16 rounded-full flex items-center justify-center text-2xl transition-all"
                      style={{
                        "background-color": "var(--error)",
                        color: "white",
                        "box-shadow": "0 0 0 5px rgba(239, 68, 68, 0.2)",
                        cursor: "pointer",
                      }}
                      onClick={handleToggle}
                    >
                      ⏹
                    </button>
                  }
                >
                  <button
                    class="w-16 h-16 rounded-full flex items-center justify-center text-2xl transition-all hover:scale-105"
                    style={{
                      "background-color": "var(--accent)",
                      color: "white",
                      "box-shadow": "0 4px 14px rgba(233, 69, 96, 0.3)",
                      cursor: "pointer",
                    }}
                    onClick={handleToggle}
                    disabled={state().status === "transcribing"}
                  >
                    🎤
                  </button>
                </Show>
              </div>
              <p class="text-center text-sm" style={{ color: "var(--text-secondary)" }}>
                <Show when={state().status === "recording"} fallback={
                  <Show when={state().status === "transcribing"} fallback="Click or press your hotkey to record">
                    Transcribing...
                  </Show>
                }>
                  Listening... speak now
                </Show>
              </p>
            </section>
          </Show>

          {/* ── No model warning ──────────────────────────────────── */}
          <Show when={initialModelCheck() && !context().modelReady && state().status !== "downloading"}>
            <div class="rounded-xl border p-4 flex items-start gap-3" style={{ "background-color": "rgba(239, 68, 68, 0.06)", "border-color": "rgba(239, 68, 68, 0.2)" }}>
              <span class="text-lg leading-none mt-0.5">🎙️</span>
              <div class="flex-1">
                <p class="text-sm font-medium" style={{ color: "var(--error)" }}>Model not ready</p>
                <Show
                  when={selectedModel() === CUSTOM_MODEL_ID}
                  fallback={
                    <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                      Download a speech recognition model above to start transcribing.
                    </p>
                  }
                >
                  <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                    Point Canario at your local model files above (encoder, decoder, tokens — plus
                    joiner.int8.onnx next to the encoder) to start transcribing.
                  </p>
                </Show>
              </div>
            </div>
          </Show>

          {/* ── Last transcription ────────────────────────────────── */}
          <Show when={context().lastTranscription}>
            <section class="rounded-xl border p-5" style={sectionStyle}>
              <div class="flex items-center justify-between mb-2">
                <h2 class={sectionHeader} style={sectionHeaderStyle}>Last Transcription</h2>
                <button
                  class="text-xs px-2 py-1 rounded-md hover:opacity-80 transition-opacity"
                  style={{ color: "var(--text-secondary)", cursor: "pointer", "background-color": "var(--bg)" }}
                  onClick={() => {
                    if (context().lastTranscription) {
                      navigator.clipboard.writeText(context().lastTranscription!);
                      showToast("Copied to clipboard", "success", 2000);
                    }
                  }}
                  title="Copy to clipboard"
                >
                  📋 Copy
                </button>
              </div>
              <p class="text-base leading-relaxed">"{context().lastTranscription}"</p>
              <Show when={context().lastDuration}>
                <p class="text-sm mt-2" style={{ color: "var(--text-secondary)" }}>
                  {context().lastDuration?.toFixed(1)}s
                </p>
              </Show>
            </section>
          </Show>

          {/* ── Hotkey ────────────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Hotkey</h2>
            <HotkeyCapture value={hotkey()} onChange={handleHotkeyChange} />
            <p class="text-xs mt-2" style={{ color: "var(--text-secondary)" }}>
              Press-and-hold to record. Release to stop and transcribe.
            </p>
          </section>

          {/* ── Behavior ──────────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Behavior</h2>
            <div class="flex flex-col gap-4">
              {/* Auto-paste */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Auto-paste transcription</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Automatically paste result into focused app</p>
                </div>
                <Toggle checked={autoPaste()} onChange={(v) => handleConfigToggle("auto_paste", v)} />
              </div>

              {/* Sound effects */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Sound effects</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Play sounds on recording start/stop</p>
                </div>
                <Toggle checked={soundEffects()} onChange={(v) => handleConfigToggle("sound_effects", v)} />
              </div>

              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Show tray icon</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Show Canario in the system tray</p>
                </div>
                <Toggle checked={showTrayIcon()} onChange={(v) => handleConfigToggle("show_tray_icon", v)} />
              </div>

              {/* Autostart */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Start on login</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Launch Canario when you log in</p>
                </div>
                <Toggle checked={autostart()} onChange={(v) => handleConfigToggle("autostart", v)} />
              </div>

              {/* Audio during recording */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Audio during recording</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>System audio behavior while recording</p>
                </div>
                <select
                  class="px-3 py-1.5 rounded-lg border text-sm"
                  style={{
                    "background-color": "var(--bg)",
                    "border-color": "var(--border)",
                    color: "var(--text-primary)",
                    cursor: "pointer",
                  }}
                  value={audioBehavior()}
                  onChange={(e) => handleAudioBehaviorChange(e.currentTarget.value)}
                >
                  <option value="DoNothing">Do nothing</option>
                  <option value="Mute">Mute system audio</option>
                </select>
              </div>
              <Show when={audioBehavior() === "Mute"}>
                <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                  Mute mutes the default audio output via pactl (PulseAudio/PipeWire) while recording and restores
                  its previous state when the recording stops or is cancelled. If pactl isn't available (e.g. macOS,
                  Windows, or a minimal Linux install), audio simply stays on and a warning is logged.
                </p>
              </Show>
            </div>
          </section>

          {/* ── Word Remapping ────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Word Remapping</h2>
            <Show when={remappings().length > 0 || removals().length > 0} fallback={
              <div class="py-4 text-center">
                <p class="text-sm" style={{ color: "var(--text-secondary)" }}>
                  No remapping rules yet. Add rules to fix common misrecognitions.
                </p>
                <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                  e.g. "I llama" → "I'll ama"
                </p>
              </div>
            }>
              <p class="text-xs mb-3" style={{ color: "var(--text-secondary)" }}>
                Fix common misrecognitions and remove filler words
              </p>
            </Show>
            <WordRemapping
              remappings={remappings()}
              removals={removals()}
              onChange={handlePostProcessorChange}
            />
          </section>

          {/* ── Appearance ────────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Appearance</h2>
            <div class="flex gap-2">
              <For each={["dark", "light", "system"]}>
                {(t) => (
                  <button
                    class="flex-1 px-3 py-2 rounded-lg border text-sm font-medium transition-colors"
                    style={{
                      "background-color": theme() === t ? "var(--surface-hover)" : "transparent",
                      "border-color": theme() === t ? "var(--accent)" : "var(--border)",
                      color: "var(--text-primary)",
                      cursor: "pointer",
                    }}
                    onClick={() => handleThemeChange(t)}
                  >
                    {t.charAt(0).toUpperCase() + t.slice(1)}
                  </button>
                )}
              </For>
            </div>
          </section>

          {/* ── About & Updates ────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>About</h2>
            <div class="flex flex-col gap-3">
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Version</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>{appVersion()}</p>
                </div>
                <Show when={updateChecking()} fallback={
                  <button
                    class="px-3 py-1.5 rounded-lg text-xs font-medium border transition-colors hover:opacity-80"
                    style={{ "background-color": "var(--bg)", "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                    onClick={handleCheckUpdate}
                  >
                    Check for Updates
                  </button>
                }>
                  <div class="flex items-center gap-2">
                    <div class="w-3 h-3 rounded-full border-2 border-t-transparent animate-spin" style={{ "border-color": "var(--accent)", "border-top-color": "transparent" }} />
                    <span class="text-xs" style={{ color: "var(--text-secondary)" }}>Checking...</span>
                  </div>
                </Show>
              </div>
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Onboarding</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Replay the first-launch setup wizard</p>
                </div>
                <button
                  class="px-3 py-1.5 rounded-lg text-xs font-medium border transition-colors hover:opacity-80"
                  style={{ "background-color": "var(--bg)", "border-color": "var(--border)", color: "var(--text-primary)", cursor: "pointer" }}
                  onClick={handleRerunOnboarding}
                >
                  Re-run
                </button>
              </div>
              <Show when={updateAvailable()}>
                <div class="rounded-lg p-3 flex items-start gap-2" style={{ "background-color": "rgba(74, 222, 128, 0.06)", border: "1px solid rgba(74, 222, 128, 0.2)" }}>
                  <span class="text-sm leading-none mt-0.5">🎉</span>
                  <div class="flex-1">
                    <p class="text-sm font-medium" style={{ color: "var(--success)" }}>Update available: v{updateVersion()}</p>
                    <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
                      Restart Canario to install the latest version.
                    </p>
                  </div>
                </div>
              </Show>
            </div>
          </section>

          {/* ── History ───────────────────────────────────────────── */}
          <section
            ref={historySectionRef}
            id="history-section"
            tabindex="-1"
            class="rounded-xl border p-5 scroll-mt-14 outline-none"
            style={sectionStyle}
          >
            <div class="flex items-center justify-between mb-3">
              <h2 class={sectionHeader} style={sectionHeaderStyle}>History</h2>
              <Show when={history().length > 0}>
                <button
                  class="text-xs px-2 py-1 rounded-md hover:opacity-80 transition-opacity"
                  style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                  onClick={handleClearHistory}
                >
                  Clear All
                </button>
              </Show>
            </div>

            {/* Search */}
            <Show when={history().length > 0 || historySearch()}>
              <div class="mb-3">
                <input
                  type="text"
                  placeholder="🔍  Search transcriptions..."
                  value={historySearch()}
                  onInput={(e) => {
                    setHistorySearch(e.currentTarget.value);
                    // Debounce search
                    clearTimeout(searchTimeout);
                    searchTimeout = setTimeout(() => loadHistory(), 300);
                  }}
                  style={{
                    "background-color": "var(--bg)",
                    border: "1px solid var(--border)",
                    color: "var(--text-primary)",
                    "border-radius": "8px",
                    padding: "8px 12px",
                    "font-size": "13px",
                    width: "100%",
                    outline: "none",
                  }}
                />
              </div>
            </Show>

            <Show when={history().length > 0} fallback={
              <div class="py-8 text-center">
                <Show when={historySearch()} fallback={
                  <div class="flex flex-col items-center gap-2">
                    <span class="text-3xl">🎤</span>
                    <p class="text-sm" style={{ color: "var(--text-secondary)" }}>
                      No transcriptions yet
                    </p>
                    <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                      Press your hotkey and start talking!
                    </p>
                  </div>
                }>
                  <div class="flex flex-col items-center gap-2">
                    <span class="text-2xl">🔍</span>
                    <p class="text-sm" style={{ color: "var(--text-secondary)" }}>
                      No results found for “{historySearch()}”
                    </p>
                    <button
                      class="text-xs px-3 py-1 rounded-md"
                      style={{ color: "var(--accent)", cursor: "pointer", "background-color": "var(--bg)" }}
                      onClick={() => { setHistorySearch(""); loadHistory(); }}
                    >
                      Clear search
                    </button>
                  </div>
                </Show>
              </div>
            }>
              <div
                ref={setHistoryListEl}
                style={{
                  height: `${historyVirtualizer.getTotalSize()}px`,
                  width: "100%",
                  position: "relative",
                }}
              >
                <For each={historyVirtualizer.getVirtualItems()}>
                  {(virtualRow) => {
                    const entry = history()[virtualRow.index];
                    return (
                      <div
                        data-index={virtualRow.index}
                        ref={historyVirtualizer.measureElement}
                        style={{
                          position: "absolute",
                          top: 0,
                          left: 0,
                          width: "100%",
                          transform: `translateY(${virtualRow.start - historyVirtualizer.options.scrollMargin}px)`,
                        }}
                      >
                        <div
                          class="group p-3 rounded-lg border transition-colors"
                          classList={{
                            "animate-slide-left": deletingIds().has(entry.id),
                          }}
                          style={{ "background-color": "var(--bg)", "border-color": "var(--border)" }}
                        >
                          <p class="text-sm leading-relaxed">{entry.text}</p>
                          <div class="flex items-center justify-between mt-1.5">
                            <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                              {entry.duration_secs.toFixed(1)}s · {formatTimestamp(entry.timestamp)}
                            </p>
                            <div class="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
                              <button
                                class="text-xs px-1.5 py-0.5 rounded hover:opacity-80 transition-opacity"
                                style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                                onClick={() => {
                                  navigator.clipboard.writeText(entry.text);
                                  showToast("Copied to clipboard", "success", 2000);
                                }}
                                title="Copy"
                              >
                                📋
                              </button>
                              <button
                                class="text-xs px-1.5 py-0.5 rounded hover:opacity-80 transition-opacity"
                                style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                                onClick={() => handleDeleteHistory(entry.id)}
                                title="Delete"
                              >
                                🗑️
                              </button>
                            </div>
                          </div>
                        </div>
                      </div>
                    );
                  }}
                </For>
              </div>
            </Show>
          </section>

        </div>
      </Show>

      {/* Toast notification container */}
      <ToastContainer />
    </div>
  );
}

// Debounce timer for history search
let searchTimeout: ReturnType<typeof setTimeout>;

/** Format an ISO timestamp to a relative/local string */
function formatTimestamp(ts: string): string {
  const date = new Date(ts);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return "Just now";
  if (diffMins < 60) return `${diffMins} min ago`;
  if (diffHours < 24) return `Today at ${date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
  if (diffDays === 1) return `Yesterday at ${date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" }) + ` at ${date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}

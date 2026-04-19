// Settings / Main page for Canario Electron
import { createSignal, Show, For, onMount, createEffect } from "solid-js";
import { useAppState } from "../state/context";
import { createCanario } from "../primitives/createCanario";
import { HotkeyCapture, toAccelerator } from "../components/HotkeyCapture";
import { WordRemapping } from "../components/WordRemapping";

const MODELS = [
  { id: "ParakeetV3", name: "Parakeet TDT v3", desc: "Multilingual · ~640MB" },
  { id: "ParakeetV2", name: "Parakeet TDT v2", desc: "English only · ~640MB" },
] as const;

type HistoryEntry = { id: string; text: string; duration_secs: number; timestamp: string };

export function AppPage() {
  const machine = useAppState();
  const canario = createCanario(machine);
  const { state, context, updateContext } = machine;

  const [history, setHistory] = createSignal<HistoryEntry[]>([]);
  const [selectedModel, setSelectedModel] = createSignal("ParakeetV3");
  const [downloadedModels, setDownloadedModels] = createSignal<Set<string>>(new Set());
  const [loading, setLoading] = createSignal(true);
  const [historySearch, setHistorySearch] = createSignal("");
  const [theme, setTheme] = createSignal<string>("dark");

  // Config state
  const [hotkey, setHotkey] = createSignal<string[]>([]);
  const [autoPaste, setAutoPaste] = createSignal(true);
  const [soundEffects, setSoundEffects] = createSignal(true);
  const [autostart, setAutostart] = createSignal(false);
  const [audioBehavior, setAudioBehavior] = createSignal<string>("DoNothing");
  const [remappings, setRemappings] = createSignal<{ from: string; to: string }[]>([]);
  const [removals, setRemovals] = createSignal<{ word: string }[]>([]);
  const [platform, setPlatform] = createSignal<{ isLinux: boolean; isMac: boolean; isWindows: boolean }>({ isLinux: true, isMac: false, isWindows: false });

  async function checkModelDownloaded(modelId: string): Promise<boolean> {
    await canario.updateConfig({ model: modelId });
    const res = await canario.command("is_model_downloaded");
    return !!(res?.ok) && res.data === true;
  }

  // Apply theme
  function applyTheme(t: string) {
    const root = document.documentElement;
    if (t === "light") {
      root.style.setProperty("color-scheme", "light");
      root.setAttribute("data-theme", "light");
    } else if (t === "system") {
      root.style.removeProperty("color-scheme");
      root.removeAttribute("data-theme");
    } else {
      root.style.setProperty("color-scheme", "dark");
      root.setAttribute("data-theme", "dark");
    }
  }

  createEffect(() => {
    applyTheme(theme());
  });

  async function loadHistory() {
    const query = historySearch().trim();
    let res: Record<string, unknown> | null;
    if (query) {
      res = await canario.searchHistory(query);
    } else {
      res = await canario.getHistory(50);
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
        setAudioBehavior((config.recording_audio_behavior as string) || "DoNothing");

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

      // Restore original model selection
      const originalModel = (cfg as Record<string, unknown>)?.model as string || "ParakeetV3";
      await canario.updateConfig({ model: originalModel });
      setSelectedModel(originalModel);

      // 3. Update app state
      const currentReady = downloadedModels().has(originalModel);
      updateContext({ modelReady: currentReady });

      // 4. Auto-load history
      await loadHistory();
    } catch (err) {
      console.error("[AppPage] init error:", err);
    }
    setLoading(false);
  });

  async function handleToggle() {
    await canario.toggleRecording();
  }

  async function handleSelectModel(modelId: string) {
    setSelectedModel(modelId);
    await canario.updateConfig({ model: modelId });
    const ready = downloadedModels().has(modelId);
    updateContext({ modelReady: ready });
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
    if (field === "autostart") setAutostart(value);
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
    await canario.deleteHistory(id);
    await loadHistory();
  }

  async function handleClearHistory() {
    await canario.clearHistory();
    setHistory([]);
  }

  async function handleThemeChange(t: string) {
    setTheme(t);
    await canario.setTheme(t);
  }

  const currentModelDownloaded = () => downloadedModels().has(selectedModel());
  const currentModel = () => MODELS.find(m => m.id === selectedModel()) || MODELS[0];

  // Reusable styles
  const sectionStyle = { "background-color": "var(--surface)", "border-color": "var(--border)" } as const;
  const sectionHeader = "text-sm font-semibold uppercase tracking-wider mb-4";
  const sectionHeaderStyle = { color: "var(--text-secondary)" } as const;

  return (
    <div class="h-screen overflow-y-auto" style={{ "background-color": "var(--bg)", color: "var(--text-primary)" }}>
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
              class="px-4 py-1.5 rounded-lg text-sm font-medium transition-colors"
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
          <p class="text-sm" style={{ color: "var(--text-secondary)" }}>Loading…</p>
        </div>
      }>
        <div class="max-w-lg mx-auto px-5 py-6 flex flex-col gap-5">

          {/* ── Model section ─────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Model</h2>

            <div class="flex flex-col gap-2 mb-4">
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
              when={currentModelDownloaded()}
              fallback={
                <Show
                  when={state().status === "downloading"}
                  fallback={
                    <button
                      class="w-full py-2.5 rounded-lg text-sm font-medium transition-colors"
                      style={{ "background-color": "var(--accent)", color: "white", cursor: "pointer" }}
                      onClick={handleDownload}
                    >
                      Download {currentModel().name}
                    </button>
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
                </Show>
              }
            >
              <div class="flex items-center justify-between py-1">
                <span class="text-sm font-medium" style={{ color: "var(--success)" }}>
                  ✓ {currentModel().name} is ready
                </span>
                <button
                  class="text-xs px-2 py-1 rounded-md"
                  style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                  onClick={async () => {
                    await canario.deleteModel();
                    setDownloadedModels(prev => {
                      const next = new Set(prev);
                      next.delete(selectedModel());
                      return next;
                    });
                    updateContext({ modelReady: false });
                  }}
                >
                  Delete
                </button>
              </div>
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
                    Transcribing…
                  </Show>
                }>
                  Listening… speak now
                </Show>
              </p>
            </section>
          </Show>

          {/* ── Last transcription ────────────────────────────────── */}
          <Show when={context().lastTranscription}>
            <section class="rounded-xl border p-5" style={sectionStyle}>
              <div class="flex items-center justify-between mb-2">
                <h2 class={sectionHeader} style={sectionHeaderStyle}>Last Transcription</h2>
                <button
                  class="text-xs px-2 py-1 rounded-md"
                  style={{ color: "var(--text-secondary)", cursor: "pointer", "background-color": "var(--bg)" }}
                  onClick={() => {
                    if (context().lastTranscription) {
                      navigator.clipboard.writeText(context().lastTranscription!);
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

          {/* ── Error ─────────────────────────────────────────────── */}
          <Show when={context().lastError}>
            <div class="rounded-xl border p-4" style={{ "background-color": "rgba(239, 68, 68, 0.08)", "border-color": "rgba(239, 68, 68, 0.2)" }}>
              <p class="text-sm" style={{ color: "var(--error)" }}>⚠ {context().lastError}</p>
            </div>
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
            <div class="flex flex-col gap-3">
              {/* Auto-paste */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Auto-paste transcription</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Automatically paste result into focused app</p>
                </div>
                <label class="relative inline-flex cursor-pointer">
                  <input type="checkbox" class="sr-only peer" checked={autoPaste()} onChange={(e) => handleConfigToggle("auto_paste", e.target.checked)} />
                  <div
                    class="w-10 h-5 rounded-full transition-colors"
                    style={{
                      "background-color": autoPaste() ? "var(--accent)" : "var(--border)",
                    }}
                  >
                    <div
                      class="absolute top-0.5 w-4 h-4 rounded-full transition-all duration-200"
                      style={{
                        "background-color": "white",
                        left: autoPaste() ? "22px" : "2px",
                      }}
                    />
                  </div>
                </label>
              </div>

              {/* Sound effects */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Sound effects</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Play sounds on recording start/stop</p>
                </div>
                <label class="relative inline-flex cursor-pointer">
                  <input type="checkbox" class="sr-only peer" checked={soundEffects()} onChange={(e) => handleConfigToggle("sound_effects", e.target.checked)} />
                  <div class="w-10 h-5 rounded-full transition-colors" style={{ "background-color": soundEffects() ? "var(--accent)" : "var(--border)" }}>
                    <div
                      class="absolute top-0.5 w-4 h-4 rounded-full transition-all duration-200"
                      style={{
                        "background-color": "white",
                        left: soundEffects() ? "22px" : "2px",
                      }}
                    />
                  </div>
                </label>
              </div>

              {/* Autostart */}
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">Start on login</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>Launch Canario when you log in</p>
                </div>
                <label class="relative inline-flex cursor-pointer">
                  <input type="checkbox" class="sr-only peer" checked={autostart()} onChange={(e) => handleConfigToggle("autostart", e.target.checked)} />
                  <div class="w-10 h-5 rounded-full transition-colors" style={{ "background-color": autostart() ? "var(--accent)" : "var(--border)" }}>
                    <div
                      class="absolute top-0.5 w-4 h-4 rounded-full transition-all duration-200"
                      style={{
                        "background-color": "white",
                        left: autostart() ? "22px" : "2px",
                      }}
                    />
                  </div>
                </label>
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
            </div>
          </section>

          {/* ── Word Remapping ────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <h2 class={sectionHeader} style={sectionHeaderStyle}>Word Remapping</h2>
            <p class="text-xs mb-3" style={{ color: "var(--text-secondary)" }}>
              Fix common misrecognitions and remove filler words
            </p>
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

          {/* ── History ───────────────────────────────────────────── */}
          <section class="rounded-xl border p-5" style={sectionStyle}>
            <div class="flex items-center justify-between mb-3">
              <h2 class={sectionHeader} style={sectionHeaderStyle}>History</h2>
              <Show when={history().length > 0}>
                <button
                  class="text-xs px-2 py-1 rounded-md"
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
              <p class="text-sm py-4 text-center" style={{ color: "var(--text-secondary)" }}>
                {historySearch() ? "No results found" : "No transcriptions yet. Press your hotkey and start talking! 🎤"}
              </p>
            }>
              <div class="flex flex-col gap-2">
                <For each={history()}>
                  {(entry) => (
                    <div class="group p-3 rounded-lg border transition-colors" style={{ "background-color": "var(--bg)", "border-color": "var(--border)" }}>
                      <p class="text-sm leading-relaxed">{entry.text}</p>
                      <div class="flex items-center justify-between mt-1.5">
                        <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                          {entry.duration_secs.toFixed(1)}s · {formatTimestamp(entry.timestamp)}
                        </p>
                        <div class="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                          <button
                            class="text-xs px-1.5 py-0.5 rounded"
                            style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                            onClick={() => navigator.clipboard.writeText(entry.text)}
                            title="Copy"
                          >
                            📋
                          </button>
                          <button
                            class="text-xs px-1.5 py-0.5 rounded"
                            style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                            onClick={() => handleDeleteHistory(entry.id)}
                            title="Delete"
                          >
                            🗑️
                          </button>
                        </div>
                      </div>
                    </div>
                  )}
                </For>
              </div>
            </Show>
          </section>

        </div>
      </Show>
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

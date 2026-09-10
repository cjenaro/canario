// Sidecar IPC bridge — Solid primitive
// Connects the Electron preload API to the state machine

import { onCleanup, onMount } from "solid-js";
import type { AppMachine } from "../state/machine";

// Type for the preload-exposed API
interface CanarioAPI {
  sendCommand: (cmd: Record<string, unknown>) => Promise<Record<string, unknown>>;
  onEvent: (callback: (event: Record<string, unknown>) => void) => () => void;
  showOverlay: () => Promise<void>;
  hideOverlay: () => Promise<void>;
  showSettings: () => Promise<void>;
  registerShortcut: (accelerator: string) => Promise<boolean>;
  unregisterShortcut: () => Promise<void>;
  onHotkey: (callback: () => void) => () => void;
  getPlatform: () => Promise<{ platform: string; isMac: boolean; isWindows: boolean; isLinux: boolean }>;
  getTheme: () => Promise<string>;
  setTheme: (theme: string) => Promise<void>;
  getOnboardingCompleted: () => Promise<boolean>;
  setOnboardingCompleted: (completed: boolean) => Promise<void>;
  hideSettings: () => Promise<void>;
  autoPaste: (text: string) => Promise<boolean>;
  setAutostart: (enabled: boolean) => Promise<boolean>;
  updateConfigCache: (config: Record<string, unknown>) => Promise<void>;
  getVersion: () => Promise<{ electron: string; sidecar: string | null; mismatch: boolean }>;
  checkForUpdate: () => Promise<{ available: boolean; version?: string }>;
  pickFile: (filters?: { name: string; extensions: string[] }[]) => Promise<string | null>;
  onUpdateAvailable: (callback: (info: { version: string }) => void) => () => void;
  onNavigateHistory: (callback: () => void) => () => void;
}

declare global {
  interface Window {
    canario: CanarioAPI;
  }
}

let commandId = 0;

function nextId(): string {
  return String(++commandId);
}

export function createCanario(machine: AppMachine) {
  const { send, updateContext, state } = machine;
  const api = window.canario;

  // Send a command to the sidecar
  async function command(cmd: string, params?: Record<string, unknown>): Promise<Record<string, unknown> | null> {
    if (!api) {
      console.error("Canario API not available (not running in Electron)");
      return null;
    }
    try {
      return await api.sendCommand({ id: nextId(), cmd, ...params });
    } catch (err) {
      console.error("Sidecar command error:", err);
      return null;
    }
  }

  // Start recording
  async function startRecording() {
    const res = await command("start_recording");
    if (res?.ok) {
      send({ type: "START_RECORDING" });
      api?.showOverlay();
    }
  }

  // Stop recording
  async function stopRecording() {
    const res = await command("stop_recording");
    if (res?.ok) {
      send({ type: "STOP_RECORDING" });
    }
  }

  // Toggle recording
  async function toggleRecording(): Promise<Record<string, unknown> | null> {
    const res = await command("toggle_recording");
    if (res?.ok) {
      const recording = (res.data as { recording?: boolean })?.recording;
      if (recording) {
        send({ type: "START_RECORDING" });
        api?.showOverlay();
      } else {
        send({ type: "STOP_RECORDING" });
        // Don't hide the overlay here — RecordingStopped keeps it alive in
        // the "Transcribing…" state until TranscriptionReady / Error.
      }
    }
    return res;
  }

  // Download model
  async function downloadModel() {
    send({ type: "START_DOWNLOAD" });
    await command("download_model");
  }

  // Delete model
  async function deleteModel() {
    await command("delete_model");
    updateContext({ modelReady: false });
  }

  // Get config
  async function getConfig() {
    const res = await command("get_config");
    if (res?.ok && res.data) {
      updateContext({ config: res.data as Record<string, unknown> });
    }
    return res?.data;
  }

  // Update config
  async function updateConfig(config: Record<string, unknown>) {
    await command("update_config", { config });
    // Sync config cache with main process (so auto-paste flag stays current)
    api?.updateConfigCache(config);
  }

  // Check if model is downloaded
  async function checkModel() {
    const res = await command("is_model_downloaded");
    const ready = !!(res?.ok) && res.data === true;
    updateContext({ modelReady: ready });
    return ready;
  }

  // Get history (the store caps at 1000 entries — the virtualized list
  // handles the full cap, so fetch it all)
  async function getHistory(limit = 1000) {
    return command("get_history", { limit });
  }

  // Search history
  async function searchHistory(query: string) {
    return command("search_history", { query });
  }

  // Delete a single history entry
  async function deleteHistory(id: string) {
    return command("delete_history", { target_id: id });
  }

  // Clear all history
  async function clearHistory() {
    return command("clear_history");
  }

  // Start hotkey listener (delegates to sidecar on Linux)
  async function startHotkey() {
    await command("start_hotkey");
  }

  // Stop hotkey listener
  async function stopHotkey() {
    await command("stop_hotkey");
  }

  // Restart hotkey listener (picks up new config)
  async function restartHotkey() {
    await command("restart_hotkey");
  }

  // Register Electron global shortcut (macOS/Windows)
  async function registerShortcut(accelerator: string) {
    return api?.registerShortcut(accelerator);
  }

  // Platform info
  async function getPlatform() {
    return api?.getPlatform();
  }

  // Theme
  async function getTheme() {
    return api?.getTheme() ?? "dark";
  }

  async function setTheme(theme: string) {
    await api?.setTheme(theme);
  }

  // Onboarding completion flag
  async function getOnboardingCompleted(): Promise<boolean> {
    return (await api?.getOnboardingCompleted()) ?? true;
  }

  async function setOnboardingCompleted(completed: boolean) {
    await api?.setOnboardingCompleted(completed);
  }

  // Hide the settings window (minimize to tray)
  async function hideSettings() {
    await api?.hideSettings();
  }

  // Auto-paste
  async function autoPaste(text: string): Promise<boolean | undefined> {
    return api?.autoPaste(text);
  }

  // Autostart
  async function setAutostart(enabled: boolean): Promise<boolean | undefined> {
    return api?.setAutostart(enabled);
  }

  // Version info
  async function getVersion() {
    return api?.getVersion();
  }

  // Check for updates
  async function checkForUpdate() {
    return api?.checkForUpdate();
  }

  // File picker (custom model paths) — returns the chosen path or null
  async function pickFile(filters?: { name: string; extensions: string[] }[]): Promise<string | null> {
    return (await api?.pickFile(filters)) ?? null;
  }

  // Tray "History" item navigation
  function onNavigateHistory(callback: () => void): () => void {
    return api?.onNavigateHistory(callback) ?? (() => {});
  }

  // ── Model-download listeners ───────────────────────────────────────
  // Pages (AppPage) keep per-variant readiness (the downloadedModels set)
  // that the shared state machine doesn't know about; this hook lets them
  // refresh it when a download finishes.
  const downloadCompleteListeners = new Set<() => void>();

  function onModelDownloadComplete(callback: () => void): () => void {
    downloadCompleteListeners.add(callback);
    return () => {
      downloadCompleteListeners.delete(callback);
    };
  }

  function notifyDownloadComplete() {
    for (const listener of downloadCompleteListeners) {
      try {
        listener();
      } catch (err) {
        console.error("Model-download listener error:", err);
      }
    }
  }

  // ── Event listener ─────────────────────────────────────────────────

  onMount(() => {
    if (!api) return;

    // Listen for sidecar events
    const unsub = api.onEvent((event) => {
      const eventName = event.event as string;

      switch (eventName) {
        case "RecordingStarted":
          send({ type: "START_RECORDING" });
          api.showOverlay();
          break;

        case "RecordingStopped":
          send({ type: "RECORDING_STOPPED" });
          // Final event of every record→transcribe pipeline (also the only
          // event for too-short / no-speech recordings) — hide the overlay.
          api.hideOverlay();
          break;

        case "TranscriptionReady":
          updateContext({
            lastTranscription: event.text as string,
            lastDuration: event.duration_secs as number,
          });
          send({ type: "TRANSCRIPTION_READY" });
          api.hideOverlay();
          break;

        case "RecordingCancelled":
          // Escape-cancel: audio discarded, no TranscriptionReady follows —
          // return straight to idle and hide the overlay.
          send({ type: "RECORDING_CANCELLED" });
          api.hideOverlay();
          break;

        case "AudioLevel":
          // No-op: nothing consumes per-frame audio levels yet.
          break;

        case "Error":
          updateContext({ lastError: event.message as string });
          send({ type: "ERROR" });
          // Reset the overlay in case an error interrupted recording/transcribing
          api.hideOverlay();
          break;

        case "ModelDownloadProgress":
          send({ type: "DOWNLOAD_PROGRESS", progress: event.progress as number });
          break;

        case "ModelDownloadComplete":
          send({ type: "DOWNLOAD_COMPLETE" });
          // The event carries no model identity and the user may have
          // switched selection mid-download — re-derive readiness from
          // the core for whatever is selected NOW (Custom included).
          checkModel();
          notifyDownloadComplete();
          break;

        case "ModelDownloadFailed":
          updateContext({ lastError: event.error as string });
          send({ type: "DOWNLOAD_FAILED" });
          // Don't assume the selected model is unusable: the failure may
          // belong to a different variant than the one now selected.
          checkModel();
          break;

        case "HotkeyTriggered":
          toggleRecording();
          break;
      }
    });

    // Listen for hotkey from Electron main process (macOS/Windows)
    const unsubHotkey = api.onHotkey(() => {
      toggleRecording();
    });

    // Initial state check
    checkModel();

    onCleanup(() => {
      unsub();
      unsubHotkey();
    });
  });

    return {
      command,
      startRecording,
      stopRecording,
      toggleRecording,
      downloadModel,
      deleteModel,
      getConfig,
      updateConfig,
      checkModel,
      getHistory,
      searchHistory,
      deleteHistory,
      clearHistory,
      startHotkey,
      stopHotkey,
      restartHotkey,
      registerShortcut,
      getPlatform,
      getTheme,
      setTheme,
      getOnboardingCompleted,
      setOnboardingCompleted,
      hideSettings,
      autoPaste,
      setAutostart,
      getVersion,
      checkForUpdate,
      pickFile,
      onNavigateHistory,
      onModelDownloadComplete,
    };
}

export type CanarioBridge = ReturnType<typeof createCanario>;

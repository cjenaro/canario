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
  autoPaste: (text: string) => Promise<boolean>;
  setAutostart: (enabled: boolean) => Promise<boolean>;
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
  }

  // Check if model is downloaded
  async function checkModel() {
    const res = await command("is_model_downloaded");
    const ready = !!(res?.ok) && res.data === true;
    updateContext({ modelReady: ready });
    return ready;
  }

  // Get history
  async function getHistory(limit = 50) {
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

  // Auto-paste
  async function autoPaste(text: string): Promise<boolean | undefined> {
    return api?.autoPaste(text);
  }

  // Autostart
  async function setAutostart(enabled: boolean): Promise<boolean | undefined> {
    return api?.setAutostart(enabled);
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
          break;

        case "TranscriptionReady":
          updateContext({
            lastTranscription: event.text as string,
            lastDuration: event.duration_secs as number,
          });
          send({ type: "TRANSCRIPTION_READY" });
          // Hide overlay after a brief moment
          setTimeout(() => {
            if (state().status === "idle") {
              api.hideOverlay();
            }
          }, 500);
          break;

        case "AudioLevel":
          window.dispatchEvent(new CustomEvent("canario:audiolevel", { detail: event.level }));
          break;

        case "Error":
          updateContext({ lastError: event.message as string });
          send({ type: "ERROR" });
          break;

        case "ModelDownloadProgress":
          send({ type: "DOWNLOAD_PROGRESS", progress: event.progress as number });
          break;

        case "ModelDownloadComplete":
          updateContext({ modelReady: true });
          send({ type: "DOWNLOAD_COMPLETE" });
          break;

        case "ModelDownloadFailed":
          updateContext({ modelReady: false, lastError: event.error as string });
          send({ type: "DOWNLOAD_FAILED" });
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
    autoPaste,
    setAutostart,
  };
}

export type CanarioBridge = ReturnType<typeof createCanario>;

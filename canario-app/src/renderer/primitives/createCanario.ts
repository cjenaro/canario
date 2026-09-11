// Sidecar IPC bridge — Solid primitive
// Connects the Electron preload API to the state machine

import { onCleanup, onMount } from "solid-js";
import type { AppMachine } from "../state/machine";
import { transcriptionTransformFailed } from "./transform";

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
  getVersion: () => Promise<{
    electron: string;
    sidecar: string | null;
    mismatch: boolean;
    protocol: number | null;
    protocolMismatch: boolean;
  }>;
  checkForUpdate: () => Promise<{ available: boolean; version?: string }>;
  pickFile: (filters?: { name: string; extensions: string[] }[]) => Promise<string | null>;
  onUpdateAvailable: (callback: (info: { version: string }) => void) => () => void;
  onNavigateHistory: (callback: () => void) => () => void;
  transformStatus: () => Promise<Record<string, unknown> | null>;
  transformTest: () => Promise<Record<string, unknown> | null>;
  setTransformKey: (key: string) => Promise<{ ok: boolean; stored: boolean; error?: string } | null>;
}

declare global {
  interface Window {
    canario: CanarioAPI;
  }
}

export function createCanario(machine: AppMachine) {
  const { send, updateContext, state } = machine;
  const api = window.canario;

  // Send a command to the sidecar. No id is passed: the main process's
  // sendCommand generates and correlates unique ids itself
  // (canario-dmp.10) — any id here would be overridden.
  async function command(cmd: string, params?: Record<string, unknown>): Promise<Record<string, unknown> | null> {
    if (!api) {
      console.error("Canario API not available (not running in Electron)");
      return null;
    }
    try {
      return await api.sendCommand({ cmd, ...params });
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

  // Download model — returns the command response so callers can surface
  // rejections (AppPage's handleDownload shows them as a toast).
  async function downloadModel(): Promise<Record<string, unknown> | null> {
    send({ type: "START_DOWNLOAD" });
    const res = await command("download_model");
    if (!res?.ok) {
      // Rejected command (Custom models are local-only, a download is
      // already in progress, sidecar unreachable): no ModelDownload*
      // event will ever arrive, so exit `downloading` synthetically —
      // otherwise the machine wedges on a 0% progress bar with the
      // Download button gone (audit D1). Mirror the ModelDownloadFailed
      // event path: record the reason, then re-derive readiness (the
      // rejection may belong to a variant other than the one selected).
      updateContext({
        lastError: (res?.error as string) || "Model download could not be started",
      });
      send({ type: "DOWNLOAD_FAILED" });
      await checkModel();
    }
    return res;
  }

  // Delete model — true when the backend actually deleted it (truthful
  // acks, canario-dmp.7); readiness only flips on success.
  async function deleteModel(): Promise<boolean> {
    const res = await command("delete_model");
    if (!res?.ok) return false;
    updateContext({ modelReady: false });
    return true;
  }

  // Cancel the in-flight recording: audio is discarded, no
  // transcription, no paste, no history entry. RecordingCancelled
  // arrives as an event and drives the machine back to idle
  // (canario-dmp.5).
  async function cancelRecording() {
    return command("cancel_recording");
  }

  // Cancel the in-flight model download. ModelDownloadFailed arrives
  // as an event (drives DOWNLOAD_FAILED); .part files are kept so a
  // later download resumes where this one stopped.
  async function cancelDownload() {
    return command("cancel_download");
  }

  // Authoritative "is a download running" — events alone can't answer
  // this after a reload.
  async function isDownloading(): Promise<boolean | null> {
    const res = await command("is_downloading");
    return res?.ok ? (res.data === true) : null;
  }

  // Lifecycle snapshot for reconciliation.
  async function getStatus() {
    const res = await command("status");
    return res?.ok
      ? (res.data as { recording: boolean; transcribing: boolean; downloading: boolean })
      : null;
  }

  // Get config
  async function getConfig() {
    const res = await command("get_config");
    if (res?.ok && res.data) {
      updateContext({ config: res.data as Record<string, unknown> });
    }
    return res?.data;
  }

  // Update config — true when the write was acknowledged (truthful
  // acks, canario-dmp.7). The main-process cache only syncs for config
  // that was actually saved, so auto-paste/tray decisions never act on
  // a write that failed.
  async function updateConfig(config: Record<string, unknown>): Promise<boolean> {
    const res = await command("update_config", { config });
    if (!res?.ok) return false;
    // Sync config cache with main process (so auto-paste flag stays current)
    api?.updateConfigCache(config);
    return true;
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

  // Delete a single history entry — true when deleted (truthful acks,
  // canario-dmp.7). entry_id is the canonical param (the sidecar still
  // accepts the legacy target_id alias).
  async function deleteHistory(id: string): Promise<boolean> {
    const res = await command("delete_history", { entry_id: id });
    return res?.ok === true;
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

  // ── Transformation provider (canario-fgm.2) ────────────────────────
  // Status/probe delegate to the preload wrappers (plain sidecar
  // commands); the key goes to the MAIN process, which owns safeStorage
  // persistence and pushes it into the sidecar's memory (fgm.1 D2) —
  // the renderer only learns whether a key is held.
  async function transformStatus(): Promise<Record<string, unknown> | null> {
    return (await api?.transformStatus()) ?? null;
  }

  async function transformTest(): Promise<Record<string, unknown> | null> {
    return (await api?.transformTest()) ?? null;
  }

  async function setTransformKey(
    key: string,
  ): Promise<{ ok: boolean; stored: boolean; error?: string } | null> {
    return (await api?.setTransformKey(key)) ?? null;
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

  // ── Transform fallback notification (fgm.4) ────────────────────────
  // fgm.1 D5d: on timeout or any transform error the sidecar still
  // emits TranscriptionReady carrying the RAW text — dictation never
  // blocks and the raw paste already happened by the time anyone
  // hears about the failure. Subscribers (AppPage) surface one subtle
  // settings-window toast; the overlay is deliberately untouched.
  // Dormant until the core actually puts a failure flag on the event
  // (see transcriptionTransformFailed for the shapes coded against).
  const transformFallbackListeners = new Set<(event: Record<string, unknown>) => void>();

  function onTransformFallback(callback: (event: Record<string, unknown>) => void): () => void {
    transformFallbackListeners.add(callback);
    return () => {
      transformFallbackListeners.delete(callback);
    };
  }

  function notifyTransformFallback(event: Record<string, unknown>) {
    for (const listener of transformFallbackListeners) {
      try {
        listener(event);
      } catch (err) {
        console.error("Transform-fallback listener error:", err);
      }
    }
  }

  // ── Event listener ─────────────────────────────────────────────────

  onMount(() => {
    if (!api) return;

    // Reconcile with core truth (canario-dmp.5): a settings-window
    // reload resets this machine while core may be mid-recording or
    // mid-download — events fired before mount are gone forever, so
    // ask for status and sync the machine to it.
    command("status").then((res) => {
      if (res?.ok && res.data) {
        const d = res.data as {
          recording: boolean;
          transcribing: boolean;
          downloading: boolean;
        };
        send({ type: "STATUS_SYNC", ...d });
        if (d.recording) {
          // Core is mid-recording: restore the overlay the reload hid.
          api.showOverlay();
        }
      }
    });

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
          // fgm.4: a transform-failure flag on the event means the
          // text above (and the main-process paste) is the RAW
          // transcript — the D5d fallback. Notify subscribers for the
          // settings toast; nothing about the machine/paste path
          // changes. event.text is the canonical value either way
          // (D3) — no code path reads a raw field for pasting.
          if (transcriptionTransformFailed(event)) {
            notifyTransformFallback(event);
          }
          break;

        case "RecordingCancelled":
          // Escape-cancel: audio discarded, no TranscriptionReady follows —
          // return straight to idle and hide the overlay.
          send({ type: "RECORDING_CANCELLED" });
          api.hideOverlay();
          break;

        case "TranscriptionStarted":
          // Idempotent belt-and-braces (canario-dmp.9): the machine
          // already enters `transcribing` via the stop-response path
          // (stopRecording / toggleRecording on res.ok); from
          // `transcribing` this event's STOP_RECORDING is ignored by
          // the machine.
          send({ type: "STOP_RECORDING" });
          break;

        case "ConfigChanged":
          // config.json was written by this instance or an external
          // change was detected (canario-dmp.20) — pull the fresh
          // config into context (getConfig updates it) without awaiting.
          void getConfig();
          break;

        case "SidecarCrashed":
          // Backend process died mid-flight (canario-dmp.6): no terminal
          // core event will ever arrive, so force the machine idle, hide
          // the overlay, and surface the failure — lastError triggers the
          // AppPage error toast automatically.
          updateContext({
            lastError: `Speech backend exited unexpectedly (code ${event.code ?? "unknown"}) — please restart Canario`,
          });
          send({ type: "SIDECAR_CRASHED" });
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
      cancelRecording,
      downloadModel,
      cancelDownload,
      isDownloading,
      getStatus,
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
      onTransformFallback,
      transformStatus,
      transformTest,
      setTransformKey,
    };
}

export type CanarioBridge = ReturnType<typeof createCanario>;

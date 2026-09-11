// Preload script — exposes IPC bridge to renderer via contextBridge
import { contextBridge, ipcRenderer } from "electron";

const api = {
  // Send a command to the sidecar, returns the response promise
  sendCommand: (cmd: Record<string, unknown>) =>
    ipcRenderer.invoke("sidecar:command", cmd),

  // Listen for sidecar events
  onEvent: (callback: (event: Record<string, unknown>) => void) => {
    const handler = (_e: Electron.IpcRendererEvent, event: Record<string, unknown>) => callback(event);
    ipcRenderer.on("sidecar:event", handler);
    return () => ipcRenderer.removeListener("sidecar:event", handler);
  },

  // Overlay control
  showOverlay: () => ipcRenderer.invoke("overlay:show"),
  hideOverlay: () => ipcRenderer.invoke("overlay:hide"),

  // Overlay status pushed from the main process (e.g. "transcribing" after
  // a successful stop command — the sidecar emits no event for that phase)
  onOverlayStatus: (callback: (status: string) => void) => {
    const handler = (_e: Electron.IpcRendererEvent, status: string) => callback(status);
    ipcRenderer.on("overlay:status", handler);
    return () => ipcRenderer.removeListener("overlay:status", handler);
  },

  // ── Overlay drag affordance (canario-aud.1) ─────────────────────────────
  // The overlay page reports the island's rect; the main process enables
  // mouse events on the click-through window only while the cursor is
  // inside it (hover detection runs main-side because forwarded mouse
  // moves don't work on Linux — electron#16777).

  // Report the island's current rect (window-relative client coords) or
  // null when the island is hidden. Fire-and-forget.
  setOverlayIslandRect: (rect: { x: number; y: number; width: number; height: number } | null) =>
    ipcRenderer.send("overlay:island-rect", rect),

  // Pushed after each overlay:show — which display the window landed on
  // (id keys the per-monitor placement in AppConfig.overlay_offsets)
  onOverlayDisplay: (callback: (info: { id: string }) => void) => {
    const handler = (_e: Electron.IpcRendererEvent, info: { id: string }) => callback(info);
    ipcRenderer.on("overlay:display", handler);
    return () => ipcRenderer.removeListener("overlay:display", handler);
  },

  // Pushed when the main process toggles the overlay window between
  // click-through and interactive — true while the cursor is over the
  // island (the only moment pointer handlers can fire)
  onOverlayInteractive: (callback: (interactive: boolean) => void) => {
    const handler = (_e: Electron.IpcRendererEvent, interactive: boolean) => callback(interactive);
    ipcRenderer.on("overlay:interactive", handler);
    return () => ipcRenderer.removeListener("overlay:interactive", handler);
  },

  // Window control
  showSettings: () => ipcRenderer.invoke("window:showSettings"),
  hideSettings: () => ipcRenderer.invoke("window:hideSettings"),

  // Global shortcuts (macOS/Windows)
  registerShortcut: (accelerator: string) => ipcRenderer.invoke("shortcut:register", accelerator),
  unregisterShortcut: () => ipcRenderer.invoke("shortcut:unregister"),

  // Hotkey triggered from main process
  onHotkey: (callback: () => void) => {
    const handler = () => callback();
    ipcRenderer.on("hotkey:triggered", handler);
    return () => ipcRenderer.removeListener("hotkey:triggered", handler);
  },

  // Platform info
  getPlatform: () => ipcRenderer.invoke("app:platform"),

  // Theme
  getTheme: () => ipcRenderer.invoke("theme:get"),
  setTheme: (theme: string) => ipcRenderer.invoke("theme:set", theme),

  // Onboarding completion flag (persisted in the sidecar-owned AppConfig
  // via get_config/update_config — canario-xv9)
  getOnboardingCompleted: () => ipcRenderer.invoke("onboarding:get"),
  setOnboardingCompleted: (completed: boolean) => ipcRenderer.invoke("onboarding:set", completed),

  // Auto-paste (clipboard + simulated keystroke)
  autoPaste: (text: string) => ipcRenderer.invoke("auto-paste", text),

  // Autostart on login
  setAutostart: (enabled: boolean) => ipcRenderer.invoke("app:setAutostart", enabled),

  // Update config cache in main process (so auto-paste flag stays in sync)
  updateConfigCache: (config: Record<string, unknown>) => ipcRenderer.invoke("config:update-cache", config),

  // Version info
  getVersion: () => ipcRenderer.invoke("app:version"),

  // Manual update check
  checkForUpdate: () => ipcRenderer.invoke("app:checkUpdate"),

  // File picker (custom model paths) — returns the chosen path or null
  pickFile: (filters?: { name: string; extensions: string[] }[]) =>
    ipcRenderer.invoke("dialog:pickFile", filters),

  // Listen for update-downloaded event from main process
  onUpdateAvailable: (callback: (info: { version: string }) => void) => {
    const handler = (_e: Electron.IpcRendererEvent, info: { version: string }) => callback(info);
    ipcRenderer.on("update:available", handler);
    return () => ipcRenderer.removeListener("update:available", handler);
  },

  // Tray "History" item — scroll the settings window to the History section
  onNavigateHistory: (callback: () => void) => {
    const handler = () => callback();
    ipcRenderer.on("navigate:history", handler);
    return () => ipcRenderer.removeListener("navigate:history", handler);
  },
};

export type CanarioAPI = typeof api;

contextBridge.exposeInMainWorld("canario", api);

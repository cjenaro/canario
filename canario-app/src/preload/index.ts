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

  // Auto-paste (clipboard + simulated keystroke)
  autoPaste: (text: string) => ipcRenderer.invoke("auto-paste", text),

  // Autostart on login
  setAutostart: (enabled: boolean) => ipcRenderer.invoke("app:setAutostart", enabled),
};

export type CanarioAPI = typeof api;

contextBridge.exposeInMainWorld("canario", api);

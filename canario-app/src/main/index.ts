// Canario Electron — main process entry
import { app, BrowserWindow, globalShortcut, ipcMain, nativeImage, screen } from "electron";
import { join } from "path";
import { createTray, setSettingsWindow, updateTrayMenu } from "./tray.js";
import { startSidecar, stopSidecar, sendCommand, onSidecarEvent } from "./sidecar.js";
import { loadWindowState, saveWindowState, trackWindowState } from "./windowState.js";
import { setAutostart } from "./autostart.js";
import { autoPasteText } from "./autoPaste.js";

let mainWindow: BrowserWindow | null = null;
let overlayWindow: BrowserWindow | null = null;

const isDev = !app.isPackaged;

function createMainWindow() {
  const savedState = loadWindowState();
  const bounds = screen.getPrimaryDisplay().workAreaSize;

  mainWindow = new BrowserWindow({
    width: savedState.width || 520,
    height: savedState.height || 700,
    x: savedState.x,
    y: savedState.y,
    maxWidth: 600,
    minWidth: 400,
    resizable: true,
    show: true,
    titleBarStyle: "hidden",
    title: "Canario",
    backgroundColor: "#1a1a2e",
    webPreferences: {
      preload: join(__dirname, "../preload/index.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });

  if (savedState.isMaximized) {
    mainWindow.maximize();
  }

  // Persist window state on changes
  trackWindowState(mainWindow);

  mainWindow.on("close", (e) => {
    e.preventDefault();
    saveWindowState(mainWindow!);
    mainWindow?.hide();
    // macOS: hide from Dock when settings window closes
    if (process.platform === "darwin") {
      app.dock?.hide();
    }
  });

  if (isDev) {
    mainWindow.loadURL("http://localhost:5173");
  } else {
    mainWindow.loadFile(join(__dirname, "../renderer/index.html"));
  }
}

function createOverlayWindow() {
  const display = screen.getPrimaryDisplay();
  const { width, height } = display.workAreaSize;

  overlayWindow = new BrowserWindow({
    width: width,
    height: height,
    frame: false,
    transparent: true,
    alwaysOnTop: true,
    focusable: false,
    skipTaskbar: true,
    resizable: false,
    show: false,
    x: 0,
    y: 0,
    webPreferences: {
      preload: join(__dirname, "../preload/index.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });

  // Click-through so the overlay doesn't block interaction with windows below
  overlayWindow.setIgnoreMouseEvents(true);

  if (isDev) {
    overlayWindow.loadURL("http://localhost:5173/#overlay");
  } else {
    overlayWindow.loadFile(join(__dirname, "../renderer/index.html"), { hash: "overlay" });
  }
}

// ── IPC handlers ─────────────────────────────────────────────────────────

// Send command to sidecar
ipcMain.handle("sidecar:command", async (_e, cmd: Record<string, unknown>) => {
  return sendCommand(cmd);
});

// Show/hide overlay
ipcMain.handle("overlay:show", () => {
  // Full-screen overlay — position doesn't matter, CSS handles placement
  overlayWindow?.showInactive();
});

ipcMain.handle("overlay:hide", () => {
  overlayWindow?.hide();
});

// Show settings window
ipcMain.handle("window:showSettings", () => {
  mainWindow?.show();
  mainWindow?.focus();
  // macOS: show in Dock when settings window opens
  if (process.platform === "darwin") {
    app.dock?.show();
  }
});

ipcMain.handle("window:hideSettings", () => {
  mainWindow?.hide();
});

// Get platform info
ipcMain.handle("app:platform", () => ({
  platform: process.platform,
  isMac: process.platform === "darwin",
  isWindows: process.platform === "win32",
  isLinux: process.platform === "linux",
}));

// Theme preference persistence
ipcMain.handle("theme:get", () => {
  try {
    const path = join(app.getPath("userData"), "theme.json");
    const { readFileSync, existsSync } = require("fs");
    if (existsSync(path)) {
      return JSON.parse(readFileSync(path, "utf-8")).theme;
    }
  } catch { /* ignore */ }
  return "dark"; // default
});

ipcMain.handle("theme:set", (_e, theme: string) => {
  try {
    const { writeFileSync } = require("fs");
    const path = join(app.getPath("userData"), "theme.json");
    writeFileSync(path, JSON.stringify({ theme }));
  } catch { /* ignore */ }
});

// Auto-paste: copy text to clipboard + simulate Ctrl/Cmd+V
ipcMain.handle("auto-paste", async (_e, text: string) => {
  return autoPasteText(text);
});

// Global shortcut for macOS/Windows
ipcMain.handle("shortcut:register", async (_e, accelerator: string) => {
  globalShortcut.unregisterAll();
  try {
    return globalShortcut.register(accelerator, () => {
      mainWindow?.webContents.send("hotkey:triggered");
      overlayWindow?.webContents.send("hotkey:triggered");
      sendCommand({ id: "hotkey", cmd: "toggle_recording" });
    });
  } catch {
    return false;
  }
});

ipcMain.handle("shortcut:unregister", () => {
  globalShortcut.unregisterAll();
});

// Autostart on login
ipcMain.handle("app:setAutostart", async (_e, enabled: boolean) => {
  return setAutostart(enabled);
});

// ── App lifecycle ────────────────────────────────────────────────────────

app.whenReady().then(async () => {
  // Start sidecar
  await startSidecar();

  // Forward sidecar events to all renderer windows
  onSidecarEvent((event) => {
    // Update tray based on events
    if (event.event === "RecordingStarted") {
      updateTrayState("recording");
    } else if (event.event === "TranscriptionReady" || event.event === "RecordingStopped") {
      updateTrayState("transcribing");
    } else if (event.event === "Error") {
      updateTrayState("idle");
    }

    // Auto-paste (all platforms: Linux via xdotool ctrl+v, macOS/Windows via robotjs)
    // The sidecar no longer auto-pastes — Electron handles it for better reliability.
    if (event.event === "TranscriptionReady" && event.text) {
      const config = cachedConfig;
      if (config?.auto_paste) {
        autoPasteText(event.text as string).catch((err) => {
          console.error("[main] Auto-paste failed:", err);
        });
      }
    }

    mainWindow?.webContents.send("sidecar:event", event);
    overlayWindow?.webContents.send("sidecar:event", event);
  });

  // Fetch config from sidecar (for auto-paste flag, etc.)
  fetchConfig();

  createMainWindow();
  createOverlayWindow();
  setSettingsWindow(mainWindow);

  // macOS: hide from Dock — app lives in system tray
  if (process.platform === "darwin") {
    app.dock?.hide();
  }

  // Create tray (needs windows to exist)
  createTray();

  // Start sidecar hotkey listener on Linux
  if (process.platform === "linux") {
    sendCommand({ id: "init-hotkey", cmd: "start_hotkey" }).catch(() => {
      console.warn("Failed to start hotkey listener (may need permissions)");
    });
  }
});

// Don't quit when windows close — app lives in tray
app.on("window-all-closed", () => {});

// Clean shutdown on quit
app.on("will-quit", () => {
  stopSidecar();
  globalShortcut.unregisterAll();
});

// ── Config cache ────────────────────────────────────────────────────────
// Cache sidecar config so the main process can check auto_paste, etc.
let cachedConfig: Record<string, unknown> | null = null;

async function fetchConfig() {
  try {
    const res = await sendCommand({ id: "init-config", cmd: "get_config" });
    if (res?.ok && res.data) {
      cachedConfig = res.data as Record<string, unknown>;
    }
  } catch {
    // Config fetch is non-critical
  }
}

ipcMain.handle("config:update-cache", (_e, config: Record<string, unknown>) => {
  cachedConfig = config;
});

// Tray state updates from sidecar events
function updateTrayState(state: "idle" | "recording" | "transcribing") {
  updateTrayMenu(state);
  // After a transcription completes, go back to idle after a beat
  if (state === "transcribing") {
    setTimeout(() => updateTrayMenu("idle"), 2000);
  }
}

// ── Signal handling — prevent orphaned processes ────────────────────────
let isQuitting = false;

function forceQuit() {
  if (isQuitting) return;
  isQuitting = true;
  stopSidecar();
  globalShortcut.unregisterAll();
  app.exit(0);
}

process.on("SIGTERM", () => forceQuit());
process.on("SIGINT", () => forceQuit());

const ppid = process.ppid;
setInterval(() => {
  try {
    process.kill(ppid, 0);
  } catch {
    console.log("[canario] Parent process died, shutting down...");
    forceQuit();
  }
}, 2000);

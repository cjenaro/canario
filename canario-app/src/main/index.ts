// Canario Electron — main process entry
import { app, BrowserWindow, dialog, globalShortcut, ipcMain, nativeImage, screen } from "electron";
import { join } from "path";
import { createTray, setSettingsWindow, setTrayVisible, updateTrayMenu } from "./tray.js";
import { startSidecar, stopSidecar, sendCommand, onSidecarEvent, onCommandResponse } from "./sidecar.js";
import { loadWindowState, saveWindowState, trackWindowState } from "./windowState.js";
import { setAutostart } from "./autostart.js";
import { autoPasteText } from "./autoPaste.js";
import { initUpdater, cleanupUpdater, checkForUpdatesManual } from "./updater.js";
import { checkVersion, getVersionInfo } from "./version.js";

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

// Size + position the overlay to cover the display containing the cursor.
// Re-evaluated every time the overlay is shown so multi-monitor setups work.
function positionOverlayWindow() {
  if (!overlayWindow) return;
  const display = screen.getDisplayNearestPoint(screen.getCursorScreenPoint());
  const { x, y, width, height } = display.bounds;
  overlayWindow.setBounds({ x, y, width, height });
}

function createOverlayWindow() {
  const display = screen.getPrimaryDisplay();
  const { x, y, width, height } = display.bounds;

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
    x: x,
    y: y,
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
  // Full-screen overlay — re-position onto the display with the cursor each
  // time it's shown (multi-monitor), CSS handles placement within it.
  positionOverlayWindow();
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

// Onboarding completion persistence (mirrors theme persistence above).
// Stored in a small JSON file rather than the sidecar's AppConfig so the
// renderer can gate first-launch routing without a protocol change.
ipcMain.handle("onboarding:get", () => {
  try {
    const path = join(app.getPath("userData"), "onboarding.json");
    const { readFileSync, existsSync } = require("fs");
    if (existsSync(path)) {
      return !!JSON.parse(readFileSync(path, "utf-8")).completed;
    }
  } catch { /* ignore */ }
  return false; // default: onboarding not completed → first launch
});

ipcMain.handle("onboarding:set", (_e, completed: boolean) => {
  try {
    const { writeFileSync } = require("fs");
    const path = join(app.getPath("userData"), "onboarding.json");
    writeFileSync(path, JSON.stringify({ completed }));
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

// Version info
ipcMain.handle("app:version", () => getVersionInfo());

// Manual update check
ipcMain.handle("app:checkUpdate", async () => {
  return checkForUpdatesManual();
});

// File picker (e.g. custom model paths) — returns the chosen path or null
ipcMain.handle("dialog:pickFile", async (_e, filters?: { name: string; extensions: string[] }[]) => {
  if (!mainWindow) return null;
  const res = await dialog.showOpenDialog(mainWindow, {
    properties: ["openFile"],
    filters,
  });
  return res.canceled || res.filePaths.length === 0 ? null : res.filePaths[0];
});

// ── App lifecycle ────────────────────────────────────────────────────────

app.whenReady().then(async () => {
  // Start sidecar
  await startSidecar();

  // Check sidecar version matches Electron
  await checkVersion();

  // Forward sidecar events to all renderer windows
  onSidecarEvent((event) => {
    // Update tray based on events
    if (event.event === "RecordingStarted") {
      updateTrayState("recording");
    } else if (event.event === "TranscriptionReady" || event.event === "RecordingStopped") {
      updateTrayState("transcribing");
    } else if (event.event === "RecordingCancelled" || event.event === "Error") {
      // Cancel: straight back to idle — updateTrayState also cancels any
      // pending 2s "return to idle" timer from a previous transcription.
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

  // The sidecar transcribes inside its recording thread and only emits
  // TranscriptionReady / RecordingStopped once it's done — so the
  // "transcribing" phase is signalled by a successful stop COMMAND, not by
  // an event. All stop paths funnel through sendCommand here in the main
  // process (tray toggle, global shortcut, UI button, and the Linux hotkey
  // via the sidecar's HotkeyTriggered event → renderer toggle_recording).
  onCommandResponse((cmd, res) => {
    const name = cmd.cmd as string;
    const stopped =
      (name === "stop_recording" && res.ok === true) ||
      (name === "toggle_recording" &&
        res.ok === true &&
        (res.data as { recording?: boolean } | undefined)?.recording === false);
    if (stopped) {
      overlayWindow?.webContents.send("overlay:status", "transcribing");
    }
  });

  // Fetch config from sidecar (for auto-paste flag, tray visibility, etc.)
  await fetchConfig();

  createMainWindow();
  createOverlayWindow();
  setSettingsWindow(mainWindow);

  // Initialize auto-updater
  initUpdater(mainWindow);

  // macOS: hide from Dock — app lives in system tray
  if (process.platform === "darwin") {
    app.dock?.hide();
  }

  // Create tray (needs windows to exist) — respects the show_tray_icon config
  if (cachedConfig?.show_tray_icon !== false) {
    createTray();
  }

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

// The renderer sends partial config updates (only the keys that changed) —
// merge them into the cache rather than replacing it, so unrelated fields
// (e.g. auto_paste) survive an update that doesn't mention them.
ipcMain.handle("config:update-cache", (_e, config: Record<string, unknown>) => {
  cachedConfig = { ...(cachedConfig ?? {}), ...config };
  // Live-apply tray visibility when the setting changes from the settings UI
  if ("show_tray_icon" in config) {
    setTrayVisible(config.show_tray_icon !== false);
  }
});

// Tray state updates from sidecar events
let trayIdleTimer: ReturnType<typeof setTimeout> | null = null;

function updateTrayState(state: "idle" | "recording" | "transcribing") {
  // Any newer state supersedes a pending "return to idle" timer — otherwise a
  // new recording started within the 2s window would get snapped to idle.
  if (trayIdleTimer) {
    clearTimeout(trayIdleTimer);
    trayIdleTimer = null;
  }
  updateTrayMenu(state);
  // After a transcription completes, go back to idle after a beat
  if (state === "transcribing") {
    trayIdleTimer = setTimeout(() => {
      trayIdleTimer = null;
      updateTrayMenu("idle");
    }, 2000);
  }
}

// ── Signal handling — prevent orphaned processes ────────────────────────
let isQuitting = false;

function forceQuit() {
  if (isQuitting) return;
  isQuitting = true;
  cleanupUpdater();
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

// System tray icon + menu
import { Tray, Menu, nativeImage, BrowserWindow, app } from "electron";
import { join } from "path";
import { sendCommand } from "./sidecar.js";

let tray: Tray | null = null;
let settingsWindow: BrowserWindow | null = null;

/** Called from main.ts so tray can reference the settings window */
export function setSettingsWindow(win: BrowserWindow | null) {
  settingsWindow = win;
}

function getTrayIcon(): Electron.NativeImage {
  const isDev = !app.isPackaged;
  const iconPath = isDev
    ? join(__dirname, "../../resources/icon.png")
    : join(process.resourcesPath, "icon.png");

  try {
    const icon = nativeImage.createFromPath(iconPath);
    if (!icon.isEmpty()) {
      // Resize for tray (22x22 on Linux, 16x16 on macOS, scaled for HiDPI)
      return icon.resize({ width: 22, height: 22 });
    }
  } catch {
    // Fall through to empty
  }

  console.warn("[tray] No icon found, using empty image");
  return nativeImage.createEmpty();
}

export function createTray(): Tray {
  const icon = getTrayIcon();

  tray = new Tray(icon);
  tray.setToolTip("Canario — Voice to Text");

  updateTrayMenu("idle");

  return tray;
}

export function updateTrayMenu(state: "idle" | "recording" | "transcribing") {
  if (!tray) return;

  const statusLabel =
    state === "recording" ? "● Recording" :
    state === "transcribing" ? "⟳ Transcribing…" :
    "● Ready";

  const toggleLabel = state === "recording" ? "■ Stop Recording" : "▶ Start Recording";

  const contextMenu = Menu.buildFromTemplate([
    { label: statusLabel, enabled: false },
    { type: "separator" },
    {
      label: toggleLabel,
      click: () => {
        sendCommand({ id: "tray-toggle", cmd: "toggle_recording" });
      },
    },
    { type: "separator" },
    {
      label: "⚙ Settings",
      click: () => {
        settingsWindow?.show();
        settingsWindow?.focus();
      },
    },
    { type: "separator" },
    {
      label: "Quit",
      click: () => {
        sendCommand({ id: "quit", cmd: "shutdown" }).finally(() => {
          app.quit();
        });
      },
    },
  ]);

  tray.setContextMenu(contextMenu);
}

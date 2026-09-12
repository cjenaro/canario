// System tray icon + menu
import { Tray, Menu, nativeImage, BrowserWindow, app } from "electron";
import { join } from "path";
import { sendCommand } from "./sidecar.js";
import { versionWarningText } from "./version.js";
import { mainT } from "./strings.js";

let tray: Tray | null = null;
let currentState: "idle" | "recording" | "transcribing" = "idle";
let settingsWindow: BrowserWindow | null = null;
let offline = false;

/** Called from main.ts so tray can reference the settings window */
export function setSettingsWindow(win: BrowserWindow | null) {
  settingsWindow = win;
}

/**
 * Mark the backend as offline in the tooltip (canario-dmp.6): a dead
 * sidecar leaves the tray otherwise indistinguishable from a healthy
 * idle app. One-way by design — recovery requires an app restart.
 */
export function setTrayOffline(isOffline: boolean): void {
  offline = isOffline;
  if (tray) {
    tray.setToolTip(baseTooltip());
  }
}

function baseTooltip(): string {
  const warning = versionWarningText();
  const parts = [mainT("tray.tooltip.tagline")];
  if (offline) parts.push(mainT("tray.tooltip.offline"));
  else if (warning) parts.push(`⚠ ${warning}`);
  return parts.join(" ");
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
  // checkVersion() runs before the tray is created, so a version or
  // protocol mismatch is visible in the tooltip from the start — and
  // stays visible for as long as the tray lives (canario-dmp.4).
  tray.setToolTip(baseTooltip());

  updateTrayMenu(currentState);

  return tray;
}

/** Create or destroy the tray icon to match the show_tray_icon config. */
export function setTrayVisible(visible: boolean): void {
  if (visible) {
    if (!tray) createTray();
  } else if (tray) {
    tray.destroy();
    tray = null;
  }
}

/** Show + focus the settings window (and restore the Dock icon on macOS). */
function showSettingsWindow() {
  settingsWindow?.show();
  settingsWindow?.focus();
  if (process.platform === "darwin") {
    app.dock?.show();
  }
}

export function updateTrayMenu(state: "idle" | "recording" | "transcribing") {
  currentState = state;
  if (!tray) return;

  const statusLabel =
    state === "recording" ? mainT("tray.status.recording") :
    state === "transcribing" ? mainT("tray.status.transcribing") :
    mainT("tray.status.ready");

  const toggleLabel =
    state === "recording" ? mainT("tray.toggle.stop") : mainT("tray.toggle.start");

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
      label: mainT("tray.history"),
      click: () => {
        showSettingsWindow();
        // Tell the renderer to scroll to the History section (PRD §5.2)
        settingsWindow?.webContents.send("navigate:history");
      },
    },
    {
      label: mainT("tray.settings"),
      click: () => {
        showSettingsWindow();
      },
    },
    { type: "separator" },
    {
      label: mainT("tray.quit"),
      click: () => {
        sendCommand({ id: "quit", cmd: "shutdown" }).finally(() => {
          app.quit();
        });
      },
    },
  ]);

  tray.setContextMenu(contextMenu);
}

/**
 * Re-render every tray string after a locale change (canario-tts):
 * rebuilds the context menu and the tooltip against the catalog the
 * main process just switched to. Called from fetchConfig's ConfigChanged
 * path — config is the persisted locale source of truth.
 */
export function refreshTrayLocale(): void {
  if (!tray) return;
  tray.setToolTip(baseTooltip());
  updateTrayMenu(currentState);
}

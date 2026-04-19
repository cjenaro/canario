// Auto-update via electron-updater + GitHub Releases
// Checks for updates on launch + every 4 hours.
// Downloads in background, notifies user to restart.

import { app, BrowserWindow, Notification, dialog } from "electron";
import { autoUpdater } from "electron-updater";
import { join } from "path";

let updateCheckInterval: ReturnType<typeof setInterval> | null = null;
let mainWindow: BrowserWindow | null = null;

// Don't check for updates in dev mode
const isDev = !app.isPackaged;

export function initUpdater(window: BrowserWindow | null): void {
  if (isDev) {
    console.log("[updater] Skipping update checks in dev mode");
    return;
  }

  mainWindow = window;

  // Configure auto-updater
  autoUpdater.autoDownload = true; // download automatically
  autoUpdater.autoInstallOnAppQuit = true; // install on quit
  autoUpdater.channel = "latest";

  // Logging (useful for debugging)
  autoUpdater.logger = {
    info: (msg: string) => console.log("[updater]", msg),
    warn: (msg: string) => console.warn("[updater]", msg),
    error: (msg: string) => console.error("[updater]", msg),
    debug: (msg: string) => console.debug("[updater]", msg),
  };

  // ── Events ──────────────────────────────────────────────────────────

  autoUpdater.on("checking-for-update", () => {
    console.log("[updater] Checking for updates...");
  });

  autoUpdater.on("update-available", (info) => {
    console.log(`[updater] Update available: v${info.version}`);
  });

  autoUpdater.on("update-not-available", () => {
    console.log("[updater] App is up to date");
  });

  autoUpdater.on("download-progress", (progressInfo) => {
    const pct = Math.round(progressInfo.percent);
    console.log(`[updater] Downloading update: ${pct}% (${progressInfo.transferred}/${progressInfo.total} bytes)`);
  });

  autoUpdater.on("update-downloaded", (info) => {
    console.log(`[updater] Update downloaded: v${info.version}`);

    // Show a system notification
    showUpdateNotification(info.version);
  });

  autoUpdater.on("error", (err) => {
    console.error("[updater] Error:", err.message);
  });

  // ── Initial check + periodic ────────────────────────────────────────

  // Check 5 seconds after launch (don't slow down startup)
  setTimeout(() => {
    checkForUpdates();
  }, 5000);

  // Check every 4 hours
  updateCheckInterval = setInterval(() => {
    checkForUpdates();
  }, 4 * 60 * 60 * 1000);
}

export function cleanupUpdater(): void {
  if (updateCheckInterval) {
    clearInterval(updateCheckInterval);
    updateCheckInterval = null;
  }
}

async function checkForUpdates(): Promise<void> {
  try {
    await autoUpdater.checkForUpdates();
  } catch (err: any) {
    console.error("[updater] Check failed:", err.message);
  }
}

function showUpdateNotification(version: string): void {
  // Try a native notification first
  if (Notification.isSupported()) {
    const notification = new Notification({
      title: "Canario Update Available",
      body: `Version ${version} has been downloaded. Click to restart and install.`,
      icon: getIconPath(),
      silent: false,
    });

    notification.on("click", () => {
      promptRestart(version);
    });

    notification.show();
  }

  // Also send to the renderer so it can show in-app if settings window is open
  mainWindow?.webContents.send("update:available", { version });
}

function promptRestart(version: string): void {
  const result = dialog.showMessageBoxSync(mainWindow!, {
    type: "info",
    title: "Restart to Update",
    message: `Canario ${version} is ready to install.`,
    detail: "Restart now to complete the update. Your unsaved transcriptions are safe.",
    buttons: ["Restart Now", "Later"],
    defaultId: 0,
    cancelId: 1,
  });

  if (result === 0) {
    autoUpdater.quitAndInstall();
  }
}

function getIconPath(): string {
  if (app.isPackaged) {
    return join(process.resourcesPath, "icon.png");
  }
  return join(__dirname, "../../resources/icon.png");
}

/**
 * Manually check for updates (called from renderer via IPC)
 */
export async function checkForUpdatesManual(): Promise<{ available: boolean; version?: string }> {
  if (isDev) {
    return { available: false };
  }

  try {
    const result = await autoUpdater.checkForUpdates();
    if (result?.updateInfo) {
      const currentVersion = app.getVersion();
      const newVersion = result.updateInfo.version;
      return {
        available: newVersion !== currentVersion,
        version: newVersion,
      };
    }
  } catch (err: any) {
    console.error("[updater] Manual check failed:", err.message);
  }
  return { available: false };
}

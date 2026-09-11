// Autostart on login — cross-platform
// macOS/Windows: Electron's app.setLoginItemSettings(), with the
// config.autostart flag persisted through the sidecar's update_config.
// Linux: fully delegated to the sidecar's set_autostart command, which
// owns the ~/.config/autostart entry AND persists config.autostart on
// success (canario-dmp.17 — one writer, no split-brain).

import { app } from "electron";
import { sendCommand } from "./sidecar.js";

/** Enable or disable autostart on login. Returns false on failure. */
export async function setAutostart(enabled: boolean): Promise<boolean> {
  if (process.platform === "linux") {
    try {
      const resp = await sendCommand({
        cmd: "set_autostart",
        enabled,
        // exec makes the sidecar write a standalone entry pointing at
        // this binary; without it it would symlink the installed menu
        // entry. Dev runs under Electron's own binary, packaged runs
        // the app executable.
        exec: app.isPackaged ? app.getPath("exe") : process.execPath,
      });
      if (resp.ok !== true) {
        console.error("[autostart] set_autostart rejected:", resp.error);
        return false;
      }
      return true;
    } catch (err) {
      // Sidecar down or timed out (10s) — sendCommand rejects.
      console.error("[autostart] set_autostart command failed:", err);
      return false;
    }
  }

  // macOS/Windows: the OS login item is Electron's to manage.
  try {
    app.setLoginItemSettings({
      openAtLogin: enabled,
      // Electron 44 removed `openAsHidden` (only worked on macOS 12+, which
      // is no longer supported). Canario hides from the Dock once ready, so
      // the login-item launch still lands in the tray.
    });
  } catch (err) {
    console.error("[autostart] Failed to set login item:", err);
    return false;
  }

  // The settings switch derives from config.autostart, so keep the flag
  // in sync. On failure return false — the OS entry may have changed but
  // the flag write didn't; the renderer's toast/revert handles that.
  try {
    const resp = await sendCommand({
      cmd: "update_config",
      config: { autostart: enabled },
    });
    if (resp.ok !== true) {
      console.error("[autostart] update_config rejected:", resp.error);
      return false;
    }
    return true;
  } catch (err) {
    console.error("[autostart] update_config command failed:", err);
    return false;
  }
}

// Autostart on login — cross-platform
// macOS/Windows: Electron's app.setLoginItemSettings()
// Linux: .desktop file in ~/.config/autostart/

import { app } from "electron";
import { join } from "path";
import { existsSync, writeFileSync, unlinkSync, mkdirSync } from "fs";

function getLinuxAutostartDir(): string {
  const configHome = process.env.XDG_CONFIG_HOME || join(process.env.HOME || "~", ".config");
  return join(configHome, "autostart");
}

function getLinuxAutostartPath(): string {
  return join(getLinuxAutostartDir(), "canario.desktop");
}

function setLinuxAutostart(enabled: boolean): boolean {
  try {
    const autostartPath = getLinuxAutostartPath();

    if (enabled) {
      const autostartDir = getLinuxAutostartDir();
      mkdirSync(autostartDir, { recursive: true });

      // Use app.getPath("exe") for the correct binary path
      const exePath = app.isPackaged ? app.getPath("exe") : process.execPath;

      const desktopEntry = `[Desktop Entry]
Type=Application
Name=Canario
Comment=Voice-to-text
Exec=${exePath}
Icon=canario
Terminal=false
Categories=Utility;
X-GNOME-Autostart-enabled=true
Hidden=false
`;
      writeFileSync(autostartPath, desktopEntry, "utf-8");
    } else {
      if (existsSync(autostartPath)) {
        unlinkSync(autostartPath);
      }
    }
    return true;
  } catch (err) {
    console.error("[autostart] Failed to set Linux autostart:", err);
    return false;
  }
}

function setMacWindowsAutostart(enabled: boolean): boolean {
  try {
    app.setLoginItemSettings({
      openAtLogin: enabled,
      openAsHidden: true, // start in tray
    });
    return true;
  } catch (err) {
    console.error("[autostart] Failed to set login item:", err);
    return false;
  }
}

/** Enable or disable autostart on login */
export function setAutostart(enabled: boolean): boolean {
  if (process.platform === "linux") {
    return setLinuxAutostart(enabled);
  }
  return setMacWindowsAutostart(enabled);
}

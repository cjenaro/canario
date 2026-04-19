// Version management — ensure sidecar and Electron versions match
// The sidecar reports its version via ping; we compare on startup.

import { app } from "electron";
import { sendCommand } from "./sidecar.js";

let sidecarVersion: string | null = null;
let versionMismatch = false;

/**
 * Check sidecar version against Electron version on startup.
 * Stores results for later IPC queries.
 */
export async function checkVersion(): Promise<void> {
  try {
    const res = await sendCommand({ id: "version-check", cmd: "ping" });
    if (res?.ok && res.data) {
      const data = res.data as { version?: string };
      sidecarVersion = data.version || null;

      const electronVersion = app.getVersion();
      if (sidecarVersion && sidecarVersion !== electronVersion) {
        console.warn(
          `[version] Mismatch: Electron=${electronVersion}, Sidecar=${sidecarVersion}`
        );
        versionMismatch = true;
      } else {
        console.log(`[version] Aligned: v${electronVersion}`);
      }
    }
  } catch {
    console.warn("[version] Could not check sidecar version");
  }
}

/**
 * Get version info for the renderer (IPC handler).
 */
export function getVersionInfo() {
  return {
    electron: app.getVersion(),
    sidecar: sidecarVersion,
    mismatch: versionMismatch,
  };
}

// Version management — ensure sidecar and Electron versions match
// The sidecar reports its version via ping; we compare on startup.
//
// Beyond the crate/app version strings there is a wire-PROTOCOL
// compatibility version (canario-dmp.4): a sidecar whose protocol
// number differs (or is missing, i.e. predates versioning) is treated
// as incompatible — the renderer shows a persistent warning instead of
// silently misbehaving on drifted commands/events/response shapes.

import { app } from "electron";
import { sendCommand } from "./sidecar.js";
import { mainT } from "./strings.js";

// Wire-protocol compatibility version expected from the sidecar.
// Keep in sync with PROTOCOL_VERSION in canario-electron/src/main.rs —
// there is no codegen between the two sides; the pair is pinned
// together by `pin_ping_shape_and_ts_protocol_constant` in the
// sidecar's protocol tests and compared at runtime below.
export const PROTOCOL_VERSION = 1;

let sidecarVersion: string | null = null;
let sidecarProtocol: number | null = null;
let versionMismatch = false;
let protocolMismatch = false;

/**
 * Check sidecar version and protocol compatibility on startup.
 * Stores results for later IPC queries.
 */
export async function checkVersion(): Promise<void> {
  try {
    // sendCommand generates and correlates the id itself (canario-dmp.10).
    const res = await sendCommand({ cmd: "ping" });
    if (res?.ok && res.data) {
      const data = res.data as { version?: string; protocol?: number };
      sidecarVersion = data.version || null;
      sidecarProtocol =
        typeof data.protocol === "number" ? data.protocol : null;

      const electronVersion = app.getVersion();
      if (sidecarVersion && sidecarVersion !== electronVersion) {
        console.warn(
          `[version] Mismatch: Electron=${electronVersion}, Sidecar=${sidecarVersion}`
        );
        versionMismatch = true;
      } else {
        console.log(`[version] Aligned: v${electronVersion}`);
      }

      // A missing protocol number means the sidecar predates the
      // handshake — it could be arbitrarily old, so treat it as
      // incompatible rather than guessing.
      if (sidecarProtocol !== PROTOCOL_VERSION) {
        console.error(
          `[version] Protocol mismatch: app=${PROTOCOL_VERSION}, sidecar=${sidecarProtocol ?? "unknown (pre-handshake)"} — commands/events may have drifted; restart with a matching build`
        );
        protocolMismatch = true;
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
    protocol: sidecarProtocol,
    protocolMismatch,
  };
}

/** True when the sidecar speaks a different wire protocol (or none). */
export function hasProtocolMismatch(): boolean {
  return protocolMismatch;
}

/**
 * Persistent warning text when app/sidecar versions or wire protocol
 * have drifted, or null when healthy. Used by the tray tooltip; the
 * renderer builds its richer banner from getVersionInfo().
 */
export function versionWarningText(): string | null {
  if (protocolMismatch) {
    return mainT("version.protocolMismatch", {
      app: PROTOCOL_VERSION,
      sidecar: sidecarProtocol ?? "unknown (pre-handshake)",
    });
  }
  if (versionMismatch && sidecarVersion) {
    return mainT("version.staleSidecar", {
      sidecar: sidecarVersion,
      app: app.getVersion(),
    });
  }
  return null;
}

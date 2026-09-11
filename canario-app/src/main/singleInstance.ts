// Single-instance guard — only one Canario main process may run at a time.
//
// Two instances (e.g. two simultaneously-mounted AppImages) share the
// productName-derived userData dir, ~/.config/canario, the single hotkey
// socket /tmp/canario-hotkey.sock and the tray, and contend for all of it
// (bead canario-tem). The duplicate must quit before it spawns the sidecar,
// creates windows or shows the tray icon. The lock is keyed on the userData
// dir, so every installation/mount of the app contends for the same lock
// regardless of which /tmp/.mount_* tree it runs from.
import { app } from "electron";

/** Minimal window surface needed to surface the existing window again. */
export interface FocusableWindow {
  isMinimized(): boolean;
  restore(): void;
  show(): void;
  focus(): void;
}

/**
 * Bring the primary instance's window back to the foreground: restore it if
 * minimized, re-show it if hidden to tray, then focus it. On macOS also
 * re-show the Dock icon so the window isn't focused while still hidden.
 */
export function focusExistingWindow(win: FocusableWindow | null): void {
  // A second instance can be launched while the primary is still starting
  // up (e.g. while the sidecar boot is awaited), before any window exists.
  // Startup creates and shows the settings window anyway, so there is
  // nothing to surface yet — no-op rather than crash or force creation.
  if (!win) return;

  if (win.isMinimized()) {
    win.restore();
  }
  win.show();
  win.focus();
  if (process.platform === "darwin") {
    app.dock?.show();
  }
}

/**
 * Acquire Electron's single-instance lock. Returns true for the primary
 * instance (the caller should continue booting) and false for a duplicate
 * (the caller must quit immediately, before spawning the sidecar or tray).
 *
 * Call this as early as possible in the main entry. On success it also
 * wires the `second-instance` handler that fires in the primary whenever
 * another instance tried to start — the standard way to "restore/focus the
 * existing window" when the user relaunches the app.
 */
export function acquireSingleInstanceLock(
  getExistingWindow: () => FocusableWindow | null
): boolean {
  const gotLock = app.requestSingleInstanceLock();

  if (!gotLock) {
    return false;
  }

  // Registered before app.whenReady() so a duplicate launched during the
  // primary's startup is still handled: focusExistingWindow no-ops until
  // the window exists (see above).
  app.on("second-instance", () => {
    focusExistingWindow(getExistingWindow());
  });

  return true;
}

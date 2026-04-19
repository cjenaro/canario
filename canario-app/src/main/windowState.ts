// Window state persistence — remember position and size across sessions
import { app, BrowserWindow } from "electron";
import { join } from "path";
import { readFileSync, writeFileSync, existsSync } from "fs";

interface WindowState {
  x?: number;
  y?: number;
  width: number;
  height: number;
  isMaximized: boolean;
}

function getStatePath(): string {
  return join(app.getPath("userData"), "window-state.json");
}

export function loadWindowState(): Partial<WindowState> {
  try {
    const path = getStatePath();
    if (existsSync(path)) {
      const data = readFileSync(path, "utf-8");
      return JSON.parse(data);
    }
  } catch {
    // Ignore errors — fall back to defaults
  }
  return {};
}

export function saveWindowState(win: BrowserWindow): void {
  try {
    const state: WindowState = {
      width: win.getBounds().width,
      height: win.getBounds().height,
      isMaximized: win.isMaximized(),
    };

    // Only save position if not maximized
    if (!win.isMaximized() && !win.isMinimized()) {
      state.x = win.getBounds().x;
      state.y = win.getBounds().y;
    }

    writeFileSync(getStatePath(), JSON.stringify(state, null, 2));
  } catch {
    // Ignore errors — window state is non-critical
  }
}

/** Watch window events and auto-save state */
export function trackWindowState(win: BrowserWindow): void {
  const save = () => saveWindowState(win);

  win.on("resize", save);
  win.on("move", save);
  win.on("maximize", save);
  win.on("unmaximize", save);
  win.on("close", save);
}

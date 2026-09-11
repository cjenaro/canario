// Canario Electron — main process entry
import { app, BrowserWindow, dialog, globalShortcut, ipcMain, nativeImage, screen } from "electron";
import { join } from "path";
import { createTray, setSettingsWindow, setTrayOffline, setTrayVisible, updateTrayMenu } from "./tray.js";
import { startSidecar, stopSidecar, sendCommand, onSidecarEvent, onCommandResponse } from "./sidecar.js";
import { loadWindowState, saveWindowState, trackWindowState } from "./windowState.js";
import { setAutostart } from "./autostart.js";
import { autoPasteText } from "./autoPaste.js";
import { initUpdater, cleanupUpdater, checkForUpdatesManual } from "./updater.js";
import { checkVersion, getVersionInfo } from "./version.js";
import { acquireSingleInstanceLock } from "./singleInstance.js";
import { parseLegacyOnboardingFile } from "./onboarding.js";
import { decideHotkeyRouting } from "./hotkeyRouting.js";
import { initTransformCredential, saveTransformCredential } from "./transformCredential.js";
import { overlayStatusForStop } from "./overlayStatus.js";

let mainWindow: BrowserWindow | null = null;
let overlayWindow: BrowserWindow | null = null;

const isDev = !app.isPackaged;

// ── Single-instance lock ─────────────────────────────────────────────────
// Acquired before anything else (bead canario-tem): two Canario instances
// (e.g. two simultaneously-mounted AppImages) share the userData dir,
// ~/.config/canario, the single hotkey socket and the tray. A duplicate
// quits right here — before the sidecar is spawned, windows are created,
// the tray is shown or IPC handlers exist — so it never contends for any
// shared state. The primary instance instead focuses its existing window
// whenever a second instance is launched (see singleInstance.ts; a
// second-instance arriving during startup is a no-op until the window
// exists).
if (!acquireSingleInstanceLock(() => mainWindow)) {
  // Duplicate instance: quit immediately.
  app.quit();
} else {
  // ── Primary instance only ──────────────────────────────────────────────

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
      // Electron 43 defaults frameless windows to rounded corners on Linux;
      // keep the pre-43 square rendering of our custom title bar/UI.
      roundedCorners: false,
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
    // Tell the overlay page which monitor it landed on — it keys the
    // persisted overlay placement per display id (canario-aud.1).
    overlayWindow.webContents.send("overlay:display", { id: display.id.toString() });
  }

  // ── Overlay drag affordance (canario-aud.1) ─────────────────────────────
  // The overlay window is click-through so dictation never blocks the apps
  // below. Dragging routes around that: the overlay page reports the
  // island's rect, and this poll enables mouse events ONLY while the
  // cursor is over the island (so it can be grabbed and moved).
  //
  // Why polling in the main process instead of the renderer watching
  // mousemove? `setIgnoreMouseEvents(true, { forward: true })` — the
  // usual trick — forwards no mouse moves on Linux (macOS/Windows only,
  // electron#16777), and Linux is our primary platform. The cursor
  // position is already main-process-only (screen API), so the hit test
  // lives here. The renderer keeps real pointer events for the drag
  // itself once interactive.
  let overlayIslandRect: { x: number; y: number; width: number; height: number } | null = null;
  let overlayInteractive = false;
  let overlayPollTimer: ReturnType<typeof setInterval> | null = null;
  let overlayOutsidePolls = 0;
  const OVERLAY_POLL_INTERVAL_MS = 75;
  // Hysteresis: click-through resumes only after the cursor stays this
  // far outside the island for consecutive polls — a fast drag (rect
  // updates chasing the pointer) must not drop interactivity mid-flight.
  const OVERLAY_EXIT_MARGIN = 24;
  const OVERLAY_EXIT_POLLS = 2;

  function setOverlayInteractive(interactive: boolean) {
    if (!overlayWindow || interactive === overlayInteractive) return;
    overlayInteractive = interactive;
    overlayWindow.setIgnoreMouseEvents(!interactive);
    // Let the page swap in the grab cursor / enable its pointer handlers
    overlayWindow.webContents.send("overlay:interactive", interactive);
  }

  function stopOverlayPolling() {
    if (overlayPollTimer) {
      clearInterval(overlayPollTimer);
      overlayPollTimer = null;
    }
  }

  function pollOverlayHover() {
    if (!overlayWindow || !overlayIslandRect || overlayWindow.isVisible() === false) return;
    const cursor = screen.getCursorScreenPoint();
    const [winX, winY] = overlayWindow.getPosition();
    const r = overlayIslandRect;
    // The reported rect is window-relative; the window may not sit exactly
    // at its display's origin, so anchor via the window position.
    const margin = overlayInteractive ? OVERLAY_EXIT_MARGIN : 0;
    const inside =
      cursor.x >= winX + r.x - margin &&
      cursor.x <= winX + r.x + r.width + margin &&
      cursor.y >= winY + r.y - margin &&
      cursor.y <= winY + r.y + r.height + margin;
    if (inside) {
      overlayOutsidePolls = 0;
      if (!overlayInteractive) setOverlayInteractive(true);
    } else if (overlayInteractive && ++overlayOutsidePolls >= OVERLAY_EXIT_POLLS) {
      overlayOutsidePolls = 0;
      setOverlayInteractive(false);
    }
  }

  function startOverlayPolling() {
    if (overlayPollTimer) return;
    overlayPollTimer = setInterval(pollOverlayHover, OVERLAY_POLL_INTERVAL_MS);
  }

  // The overlay page pushes the island's current rect (window-relative
  // client coords, which equal display-relative offsets since the window
  // covers the display exactly). null = island hidden → stop hit-testing.
  ipcMain.on("overlay:island-rect", (_e, rect: { x: number; y: number; width: number; height: number } | null) => {
    overlayIslandRect =
      rect && Number.isFinite(rect.x) && Number.isFinite(rect.y) &&
      Number.isFinite(rect.width) && Number.isFinite(rect.height)
        ? rect
        : null;
    overlayOutsidePolls = 0;
    if (overlayIslandRect && overlayWindow?.isVisible()) {
      startOverlayPolling();
    } else {
      stopOverlayPolling();
      setOverlayInteractive(false);
    }
  });

  function createOverlayWindow() {
    const display = screen.getPrimaryDisplay();
    const { x, y, width, height } = display.bounds;

    overlayWindow = new BrowserWindow({
      width: width,
      height: height,
      frame: false,
      transparent: true,
      // Electron 43 defaults frameless windows to rounded corners on Linux —
      // the overlay must stay an exact full-screen rectangle.
      roundedCorners: false,
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
    // Never carry interactive state across hides (e.g. a drag interrupted
    // by the recording ending) — start each show fully click-through.
    setOverlayInteractive(false);
    overlayWindow?.showInactive();
  });

  ipcMain.handle("overlay:hide", () => {
    setOverlayInteractive(false);
    stopOverlayPolling();
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

  // Onboarding completion persistence (canario-xv9): the flag lives in
  // the sidecar-owned AppConfig (`onboarding_completed` in config.json),
  // read/written through the existing get_config / update_config
  // commands — no dedicated protocol command needed. Kept behind the
  // onboarding:get/set channels so the renderer surface is unchanged.
  ipcMain.handle("onboarding:get", async () => {
    try {
      const res = await sendCommand({ cmd: "get_config" });
      if (res?.ok && res.data) {
        return (res.data as { onboarding_completed?: boolean }).onboarding_completed === true;
      }
    } catch { /* ignore */ }
    return false; // default: onboarding not completed → first launch
  });

  ipcMain.handle("onboarding:set", async (_e, completed: boolean) => {
    try {
      const res = await sendCommand({
        cmd: "update_config",
        config: { onboarding_completed: completed },
      });
      if (res?.ok) {
        // Keep the main-process cache in sync (the sidecar now owns the
        // persisted value; the renderer's config:update-cache path
        // never mentions this key).
        cachedConfig = { ...(cachedConfig ?? {}), onboarding_completed: completed };
      }
      return res?.ok === true;
    } catch {
      return false;
    }
  });

  // Auto-paste: copy text to clipboard + simulate Ctrl/Cmd+V
  ipcMain.handle("auto-paste", async (_e, text: string) => {
    return autoPasteText(text);
  });

  // Transform provider API key (fgm.2 D2): persist via safeStorage
  // (userData/transform-key.bin, never config.json) and push into the
  // sidecar's memory. The renderer learns only whether a key is now
  // held — never the key itself.
  ipcMain.handle("transform:setKey", async (_e, key: unknown) => {
    return saveTransformCredential(typeof key === "string" ? key : "");
  });

  // Global shortcut for macOS/Windows
  ipcMain.handle("shortcut:register", async (_e, accelerator: string) => {
    globalShortcut.unregisterAll();
    try {
      return globalShortcut.register(accelerator, () => {
        // Exactly one toggle_recording per press (audit D9 fix): main
        // notifies, the renderer commands through its state machine. The
        // direct command below is a fallback for the no-live-window case
        // only — see hotkeyRouting.ts for the window-lifecycle reasoning.
        const routing = decideHotkeyRouting({ settings: mainWindow, overlay: overlayWindow });
        if (routing.notifySettings) {
          mainWindow?.webContents.send("hotkey:triggered");
        }
        if (routing.notifyOverlay) {
          overlayWindow?.webContents.send("hotkey:triggered");
        }
        if (routing.directToggle) {
          sendCommand({ cmd: "toggle_recording" });
        }
      });
    } catch {
      return false;
    }
  });

  ipcMain.handle("shortcut:unregister", () => {
    globalShortcut.unregisterAll();
  });

  // Autostart on login (delegates to the sidecar — see autostart.ts)
  ipcMain.handle("app:setAutostart", (_e, enabled: boolean) => setAutostart(enabled));

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

    // Push the persisted transform API key (if any) into the sidecar's
    // memory (fgm.2 D2): safeStorage decrypt here, memory-only copy
    // there. Best effort — a missing key leaves local endpoints working
    // (they need none) and the settings section re-pushes on change.
    await initTransformCredential();

    // Forward sidecar events to all renderer windows
    onSidecarEvent(async (event) => {
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
      // event.text is the CANONICAL text (fgm.1 D3): the transformed value
      // when a transform ran — and when the pass failed or timed out, the
      // sidecar already fell back to raw text (D5d), so this path needs no
      // failure branching. No code path pastes a raw field.
      if (event.event === "TranscriptionReady" && event.text) {
        // Decide against fresh config, not the boot snapshot: config.json
        // may have changed under us (GTK running concurrently, a manual
        // edit, the CLI). One local round-trip (~0.1ms, see
        // bench_get_config_roundtrip_latency in the sidecar protocol tests).
        // fetchConfig keeps the last known config on failure.
        // canario-dmp.18.
        await fetchConfig();
        const config = cachedConfig;
        if (config?.auto_paste) {
          timingMark("electron:transcript_received");
          // Never simulate a paste into our own windows: the renderer owns
          // that UI (the onboarding practice box fills itself from the
          // event), and a synthetic Ctrl+V into ourselves would rely on the
          // clipboard-propagation race it exists to avoid (stale-content
          // pastes on Wayland — see canario-fhm).
          const focused = BrowserWindow.getFocusedWindow();
          const ownWindowFocused =
            focused !== null && (focused === mainWindow || focused === overlayWindow);
          if (!ownWindowFocused) {
            timingMark("electron:paste_start");
            autoPasteText(event.text as string)
              .then(() => timingMark("electron:paste_done"))
              .catch((err) => {
                console.error("[main] Auto-paste failed:", err);
              });
          }
        }
      }

      // Backend death is a first-class event (canario-dmp.6): reset the
      // tray (its state icons would otherwise lie forever), mark it
      // offline in the tooltip, and let the generic forward below carry
      // the event to the renderer, whose machine force-resets to idle.
      if (event.event === "SidecarCrashed") {
        updateTrayState("idle");
        setTrayOffline(true);
      }

      // TranscriptionStarted (canario-dmp.9): the finished capture began
      // transcribing — push the same overlay labelling the stop-response
      // sniff in onCommandResponse below already does. Both paths stay
      // valid: the sniff covers stops routed through this process, the
      // event covers every other stop path (CLI/GTK frontend) — the
      // event is the robust one.
      if (event.event === "TranscriptionStarted") {
        overlayWindow?.webContents.send("overlay:status", overlayStatusForStop(cachedConfig));
      }

      // ConfigChanged (canario-dmp.20): config.json was written by this
      // instance or an external change was detected — consumers pull
      // get_config. Refresh the main-process cache that auto-paste and
      // tray decisions read (same freshness argument as canario-dmp.18).
      if (event.event === "ConfigChanged") {
        void fetchConfig();
      }

      // Events forward WHOLE: optional fields the core adds later — e.g.
      // fgm.3's raw_text / transform-failure flag on TranscriptionReady —
      // reach the renderer untouched; nothing here or in the preload
      // whitelists or strips fields.
      mainWindow?.webContents.send("sidecar:event", event);
      overlayWindow?.webContents.send("sidecar:event", event);
    });

    // The sidecar transcribes inside its recording thread and only emits
    // TranscriptionReady / RecordingStopped once it's done — so the
    // "transcribing"/"transforming" phase is signalled by a successful stop
    // COMMAND, not by an event. All stop paths funnel through sendCommand here
    // in the main process (tray toggle, global shortcut, UI button, and the
    // Linux hotkey via the sidecar's HotkeyTriggered event → renderer
    // toggle_recording).
    onCommandResponse((cmd, res) => {
      const name = cmd.cmd as string;
      const stopped =
        (name === "stop_recording" && res.ok === true) ||
        (name === "toggle_recording" &&
          res.ok === true &&
          (res.data as { recording?: boolean } | undefined)?.recording === false);
      if (stopped) {
        // fgm.4: with the transform block enabled, the stop→result
        // window also contains the sidecar's LLM pass (fgm.1 D1/D3 —
        // TranscriptionReady only fires after the transform settles or
        // its timeout falls back to raw, D5d). The phase boundary lives
        // in the core and is invisible here, so the whole window is
        // labelled "Transforming…"; the terminal events dismiss the
        // overlay exactly as they do for "transcribing".
        overlayWindow?.webContents.send("overlay:status", overlayStatusForStop(cachedConfig));
      }
    });

    // Fetch config from sidecar (for auto-paste flag, tray visibility, etc.)
    await fetchConfig();

    // One-time import of the legacy main-process onboarding.json flag
    // into the sidecar-owned AppConfig (canario-xv9) — before the
    // renderer's first-launch routing reads it.
    await migrateLegacyOnboarding();

    // Start the sidecar's hotkey listener on Linux BEFORE any renderer
    // window exists. The /dev/input permission probe settles
    // synchronously inside start_hotkey, so a renderer querying
    // `hotkey_status` on mount can never race it — that's what keeps
    // the permission-failure guidance from being lost to startup
    // timing (events emitted now would out-run the renderer's
    // subscription, but the status is pull-based).
    if (process.platform === "linux") {
      await sendCommand({ cmd: "start_hotkey" }).catch(() => {
        console.warn("Failed to start hotkey listener (may need permissions)");
      });
    }

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
  }).catch((err) => {
    // Startup failure must be visible, not an invisible hang: without
    // this the process idles with no windows, tray, or dialog
    // (canario-dmp.6).
    console.error("[main] Startup failed:", err);
    dialog.showErrorBox(
      "Canario failed to start",
      `The speech backend could not be started:\n\n${err instanceof Error ? err.message : String(err)}\n\nCanario will now exit.`
    );
    app.exit(1);
  });

  // Don't quit when windows close — app lives in tray
  app.on("window-all-closed", () => {});

  // Clean shutdown on quit
  app.on("will-quit", () => {
    stopOverlayPolling();
    stopSidecar();
    globalShortcut.unregisterAll();
  });

  // ── Pipeline timing marks ───────────────────────────────────────────────
  // Same schema as canario-core's timing module (CANARIO_TIMING=1):
  // one JSON line per stage, wall-clock ts_ms shared with the sidecar's
  // marks so transcript-to-paste can be measured across the process hop.
  // Marks append to CANARIO_TIMING_FILE when set (one mark stream for the
  // whole pipeline, matching scripts/bench-pipeline); otherwise they go
  // to stdout.
  const timingEnabled = !!process.env.CANARIO_TIMING;
  const timingFile = process.env.CANARIO_TIMING_FILE || null;
  function timingMark(stage: string): void {
    if (!timingEnabled) return;
    const line = JSON.stringify({ stage, ts_ms: Date.now(), pid: process.pid });
    if (timingFile) {
      try {
        const { appendFileSync } = require("fs");
        appendFileSync(timingFile, line + "\n");
        return;
      } catch (err) {
        console.error("[main] timing mark write failed:", err);
      }
    }
    console.log(line);
  }

  // ── Config cache ────────────────────────────────────────────────────────
  // Cache sidecar config so the main process can check auto_paste, etc.
  let cachedConfig: Record<string, unknown> | null = null;

  async function fetchConfig() {
    try {
      const res = await sendCommand({ cmd: "get_config" });
      if (res?.ok && res.data) {
        cachedConfig = res.data as Record<string, unknown>;
      }
    } catch {
      // Config fetch is non-critical
    }
  }

  // NOTE: any future sidecar-crash-recovery / sidecar-restart path MUST call
  // fetchConfig() again after the process comes back — the cache is only
  // refreshed at boot, on renderer config:update-cache merges, and before
  // each auto-paste decision (canario-dmp.18).

  // ── Onboarding flag migration (canario-xv9) ─────────────────────────────
  // The completion flag used to live in a main-process onboarding.json
  // (mirroring theme.json — which stays put). It now lives in the
  // sidecar-owned AppConfig, so import the legacy value once and delete
  // the file. Only a "completed" flag is imported: the AppConfig default
  // is already false, so an unfinished wizard simply stays unfinished.
  // Best-effort — if the sidecar import fails the file survives and the
  // migration retries on the next launch.
  async function migrateLegacyOnboarding() {
    try {
      const { existsSync, readFileSync, rmSync } = require("fs");
      const path = join(app.getPath("userData"), "onboarding.json");
      if (!existsSync(path)) return;
      const completed = parseLegacyOnboardingFile(readFileSync(path, "utf-8"));
      if (completed) {
        const res = await sendCommand({
          cmd: "update_config",
          config: { onboarding_completed: true },
        });
        if (!res?.ok) {
          console.warn("[main] onboarding.json import deferred (update_config failed)");
          return; // keep the file for the next launch
        }
        cachedConfig = { ...(cachedConfig ?? {}), onboarding_completed: true };
      }
      rmSync(path, { force: true });
    } catch (err) {
      console.error("[main] Legacy onboarding.json migration failed:", err);
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
}

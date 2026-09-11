// Hotkey routing decision for the macOS/Windows global shortcut
// (audit D9 fix: exactly ONE toggle_recording per press).
//
// Ownership: the RENDERER owns the toggle command. Main only notifies it
// with `hotkey:triggered`; the renderer's bridge (createCanario's onHotkey
// → toggleRecording) then issues the single toggle_recording, keeping the
// state machine in the loop. Historically main ALSO sent toggle_recording
// directly, so every press produced two commands — start + instant stop →
// zero-length "too short" recordings on macOS/Windows. Linux is unaffected:
// its hotkey arrives as a sidecar HotkeyTriggered event that only the
// renderer answers.
//
// Window lifecycle (why the direct send survives as a fallback): both
// windows are HIDDEN, never destroyed, while the app runs — the settings
// window's `close` handler is intercepted into hide() (index.ts) and
// `window-all-closed` is a no-op, so the notify path is always available
// in practice. But if some future code path leaves no live settings
// window — the only window whose page registers an onHotkey listener
// (AppPage/OnboardingPage via createCanario; OverlayPage never does) —
// notify-only would silently kill the hotkey. The fallback fires only in
// that case: main toggles directly, trading machine visibility for a
// working hotkey.
//
// Pure logic (no electron import) so the decision is unit-testable.

/** Structural slice of BrowserWindow that the routing decision needs. */
export interface HotkeyWindow {
  isDestroyed(): boolean;
}

export interface HotkeyWindows {
  /** Settings/main window — hosts the only `hotkey:triggered` listener. */
  settings: HotkeyWindow | null;
  /** Overlay window — currently has no listener; notified for parity. */
  overlay: HotkeyWindow | null;
}

export interface HotkeyRouting {
  /** Send `hotkey:triggered` to the settings window (renderer commands). */
  notifySettings: boolean;
  /** Send `hotkey:triggered` to the overlay window (no listener today). */
  notifyOverlay: boolean;
  /** Main sends toggle_recording itself — fallback when no renderer can. */
  directToggle: boolean;
}

export function decideHotkeyRouting(windows: HotkeyWindows): HotkeyRouting {
  const settingsAlive = !!windows.settings && !windows.settings.isDestroyed();
  const overlayAlive = !!windows.overlay && !windows.overlay.isDestroyed();
  return {
    notifySettings: settingsAlive,
    // Only alongside the settings window: notifying the overlay in the
    // fallback branch would risk a second toggle if the overlay ever
    // grows a listener.
    notifyOverlay: settingsAlive && overlayAlive,
    directToggle: !settingsAlive,
  };
}

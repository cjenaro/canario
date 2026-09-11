// Tests for the global-shortcut routing decision (audit D9 fix): exactly
// ONE toggle_recording per hotkey press on every platform. When a live
// settings window can receive `hotkey:triggered`, the renderer owns the
// command (see createCanario.test.ts "exactly one toggle_recording per
// press"); main toggles directly only as the no-window fallback.
import { describe, it, expect } from "vitest";
import { decideHotkeyRouting, type HotkeyWindow } from "./hotkeyRouting";

function aliveWindow(): HotkeyWindow {
  return { isDestroyed: () => false };
}

function destroyedWindow(): HotkeyWindow {
  return { isDestroyed: () => true };
}

describe("decideHotkeyRouting", () => {
  it("notifies the renderer instead of toggling directly when the settings window is alive", () => {
    const r = decideHotkeyRouting({ settings: aliveWindow(), overlay: aliveWindow() });
    expect(r).toEqual({ notifySettings: true, notifyOverlay: true, directToggle: false });
  });

  it("notifies the overlay only alongside the settings window", () => {
    // The overlay has no hotkey listener today; notifying it while main
    // also toggles directly would risk a future double-toggle.
    const r = decideHotkeyRouting({ settings: destroyedWindow(), overlay: aliveWindow() });
    expect(r.notifyOverlay).toBe(false);
  });

  it("falls back to a direct toggle when the settings window is destroyed (overlay alive or not)", () => {
    for (const overlay of [aliveWindow(), destroyedWindow(), null]) {
      const r = decideHotkeyRouting({ settings: destroyedWindow(), overlay });
      expect(r).toEqual({ notifySettings: false, notifyOverlay: false, directToggle: true });
    }
  });

  it("falls back to a direct toggle when no window exists (tray-only mode)", () => {
    const r = decideHotkeyRouting({ settings: null, overlay: null });
    expect(r).toEqual({ notifySettings: false, notifyOverlay: false, directToggle: true });
  });

  it("never notifies and directly toggles at the same time (single command per press)", () => {
    const candidates: (HotkeyWindow | null)[] = [aliveWindow(), destroyedWindow(), null];
    for (const settings of candidates) {
      for (const overlay of candidates) {
        const r = decideHotkeyRouting({ settings, overlay });
        const commandOwners = Number(r.notifySettings) + Number(r.directToggle);
        expect(commandOwners).toBe(1);
      }
    }
  });
});

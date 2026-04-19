// Cross-platform auto-paste
// All platforms: clipboard.writeText + simulated paste keystroke
//   Linux:   xdotool key ctrl+v (or wtype/ydotool)
//   macOS:   robotjs keyTap("v", "command") — requires Accessibility permissions
//   Windows: robotjs keyTap("v", "control")

import { clipboard, systemPreferences, dialog, BrowserWindow } from "electron";
import { execFile } from "child_process";

let robot: any = null;
let robotLoadAttempted = false;
let accessibilityPrompted = false;

function loadRobot(): any {
  if (robotLoadAttempted) return robot;
  robotLoadAttempted = true;

  try {
    robot = require("@jitsi/robotjs");
  } catch {
    console.warn("[autoPaste] @jitsi/robotjs not available — auto-paste via robotjs disabled");
  }
  return robot;
}

/**
 * Auto-paste text into the focused application.
 * 1. Copy text to clipboard
 * 2. Simulate paste keystroke (Ctrl+V or Cmd+V)
 *
 * Returns true if paste keystroke was attempted successfully.
 */
export async function autoPasteText(text: string): Promise<boolean> {
  if (!text) return false;

  // Always copy to clipboard first
  clipboard.writeText(text);

  // Small delay to ensure clipboard is settled before keystroke
  await new Promise((r) => setTimeout(r, 50));

  if (process.platform === "linux") {
    return linuxPaste();
  }

  // macOS / Windows: use robotjs
  const r = loadRobot();
  if (!r) {
    console.warn("[autoPaste] robotjs not loaded — text copied to clipboard but not auto-pasted");
    return false;
  }

  try {
    if (process.platform === "darwin") {
      r.keyTap("v", "command");
    } else {
      r.keyTap("v", "control");
    }
    return true;
  } catch (err) {
    console.error("[autoPaste] robotjs key tap failed:", err);
    if (process.platform === "darwin") {
      promptAccessibilityPermission();
    }
    return false;
  }
}

// ── Linux paste: try xdotool key ctrl+v, wtype, ydotool ──────────────

function linuxPaste(): Promise<boolean> {
  return new Promise((resolve) => {
    // xdotool key ctrl+v (X11)
    execFile("xdotool", ["key", "--clearmodifiers", "ctrl+v"], (err) => {
      if (!err) {
        resolve(true);
        return;
      }

      // wtype: doesn't have a "key" command for modifiers, skip to ydotool

      // ydotool key 29:1 47:1 47:0 29:0 (Ctrl down, V down, V up, Ctrl up)
      execFile("ydotool", ["key", "29:1", "47:1", "47:0", "29:0"], (err2) => {
        if (!err2) {
          resolve(true);
          return;
        }

        console.warn("[autoPaste] Linux paste failed: no xdotool or ydotool available");
        resolve(false);
      });
    });
  });
}

// ── macOS Accessibility permission prompt ──────────────────────────────

function promptAccessibilityPermission(): void {
  if (accessibilityPrompted) return;
  accessibilityPrompted = true;

  try {
    const isTrusted = systemPreferences.isTrustedAccessibilityClient(true);

    if (!isTrusted) {
      const win = BrowserWindow.getFocusedWindow();
      if (win) {
        dialog.showMessageBox(win, {
          type: "info",
          title: "Accessibility Permission Required",
          message: "Canario needs Accessibility access to auto-paste transcriptions.",
          detail:
            "To enable auto-paste:\n\n" +
            "1. Open System Settings → Privacy & Security → Accessibility\n" +
            "2. Find Canario in the list and enable it\n" +
            "3. Restart Canario\n\n" +
            "You can still use Canario without this permission — transcriptions will be copied to your clipboard.",
          buttons: ["OK"],
        });
      }
    }
  } catch {
    // systemPreferences.isTrustedAccessibilityClient may not be available on all platforms
  }
}

/**
 * Check if auto-paste is available and working.
 */
export function isAutoPasteAvailable(): boolean {
  if (process.platform === "linux") return true; // xdotool/ydotool
  return !!loadRobot();
}

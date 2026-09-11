// Cross-platform auto-paste
// Linux:   clipboard + simulated Ctrl+V first (verified read-back), typing fallback
//   macOS / Windows: clipboard (verified read-back) + the sidecar's native
//   paste chord (CoreGraphics / SendInput — canario-7x5.3, exposed as the
//   paste_text command by canario-ubb). robotjs is gone: it was the only
//   native node module, and every Electron upgrade re-broke its ABI/
//   toolchain (node-abi gap, X11 headers, libpng, MSVC discovery).

import { clipboard, systemPreferences, dialog, BrowserWindow } from "electron";
import { execFile } from "child_process";
import { sendCommand } from "./sidecar.js";

/** Marker for tests: which arm a platform takes. */
export type PasteArm = "linux-tools" | "sidecar-native";

let accessibilityPrompted = false;

/**
 * Auto-paste text into the focused application.
 * 1. Copy text to clipboard
 * 2. Simulate paste keystroke (Ctrl+V or Cmd+V)
 *
 * Returns true if paste keystroke was attempted successfully.
 */
export async function autoPasteText(text: string): Promise<boolean> {
  if (!text) return false;

  // Always copy to clipboard first: a manual Ctrl+V fallback if typing
  // fails, and users generally expect the last dictation on the clipboard.
  // Electron 44 (RFC 0019) made clipboard.writeText asynchronous — await it
  // so the later keystroke/fallback can never race the clipboard write.
  await clipboard.writeText(text);

  if (process.platform === "linux") {
    return linuxPaste(text);
  }

  // macOS / Windows: the sidecar's native paste chord (canario-ubb).
  // The clipboard must verify first — an early chord pastes whatever
  // was there before (the canario-fhm stale-clipboard bug), so an
  // unverified clipboard returns false (text stays on the clipboard
  // for a manual paste) rather than risk pasting stale content.
  await new Promise((r) => setTimeout(r, 50));
  if (!(await clipboardHoldsText(text))) {
    console.warn("[autoPaste] clipboard did not verify — skipping the paste chord");
    return false;
  }

  try {
    const res = await sendCommand({ id: `paste-${Date.now()}`, cmd: "paste_text", text });
    const pasted: boolean =
      res?.ok === true && (res.data as { pasted?: unknown } | undefined)?.pasted === true;
    if (!pasted && process.platform === "darwin") {
      // The CoreGraphics chord needs Accessibility trust — same prompt
      // the robotjs path used to show.
      promptAccessibilityPermission();
    }
    return pasted;
  } catch (err) {
    console.error("[autoPaste] sidecar paste failed:", err);
    if (process.platform === "darwin") {
      promptAccessibilityPermission();
    }
    return false;
  }
}

// ── Linux paste: clipboard + simulated Ctrl+V first, typing fallback ──

/** Linux paste tools, with the args each step kind needs. */
export type LinuxPasteTool = "wtype" | "xdotool" | "ydotool";
export type LinuxPasteStep = { kind: "ctrl_v" | "type"; tool: LinuxPasteTool };

/**
 * Ordered Linux delivery plan (canario-cy0).
 *
 * Preferred: one synthesized Ctrl+V reading the clipboard that was just
 * written and *verified* — a single round trip instead of one keystroke
 * per character. Planned only when `clipboardVerified`: with an
 * unverified clipboard the keystroke would paste stale content
 * (canario-fhm), so typing becomes the only safe strategy.
 *
 * Fallback: char-by-char typing for apps that swallow synthetic pastes
 * (and for the unverified-clipboard case).
 *
 * Tool scoping: xdotool is X11-only — under Wayland it can only reach
 * XWayland and exits 0 while the keystroke goes nowhere. wtype is
 * Wayland-only (native virtual-keyboard protocol; it has no key
 * command, so it can never synthesize Ctrl+V). ydotool goes through
 * uinput and works everywhere.
 */
export function linuxPastePlan(clipboardVerified: boolean, wayland: boolean): LinuxPasteStep[] {
  const steps: LinuxPasteStep[] = [];
  if (clipboardVerified) {
    if (!wayland) steps.push({ kind: "ctrl_v", tool: "xdotool" });
    steps.push({ kind: "ctrl_v", tool: "ydotool" });
  }
  if (wayland) steps.push({ kind: "type", tool: "wtype" });
  if (!wayland) steps.push({ kind: "type", tool: "xdotool" });
  steps.push({ kind: "type", tool: "ydotool" });
  return steps;
}

/** Args for one plan step; `text` is only needed by typing steps. */
export function linuxPasteStepArgs(step: LinuxPasteStep, text: string): string[] {
  switch (step.tool) {
    case "xdotool":
      return step.kind === "ctrl_v"
        ? ["key", "--clearmodifiers", "ctrl+v"]
        : ["type", "--clearmodifiers", "--", text];
    case "ydotool":
      // --delay 0 skips ydotool's default 100ms pre-press sleep (the
      // dominant cost of the whole paste at defaults); --key-delay 2 keeps
      // a small gap between the four chord events.
      return step.kind === "ctrl_v"
        ? ["key", "--delay", "0", "--key-delay", "2", "29:1", "47:1", "47:0", "29:0"] // Ctrl down, V down, V up, Ctrl up
        : ["type", "--delay", "0", "--", text];
    case "wtype":
      return ["--", text];
  }
}

/** Waits (ms) between clipboard read-back attempts (canario-fhm guard). */
const CLIPBOARD_READBACK_WAITS_MS = [0, 10, 25, 50];

/**
 * Verify the clipboard actually holds `text` before trusting it with a
 * paste keystroke: `clipboard.writeText` resolving is not proof the
 * compositor has published the new content, and an early Ctrl+V pastes
 * whatever was there before (canario-fhm). Short bounded read-retry;
 * false means "skip the keystroke, fall back to typing".
 *
 * `readText`/`sleep`/`waitsMs` are injected for testing.
 */
export async function clipboardHoldsText(
  text: string,
  readText: () => string | Promise<string> = () => clipboard.readText(),
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((r) => setTimeout(r, ms)),
  waitsMs: readonly number[] = CLIPBOARD_READBACK_WAITS_MS,
): Promise<boolean> {
  for (const waitMs of waitsMs) {
    if (waitMs > 0) await sleep(waitMs);
    try {
      if ((await readText()) === text) return true;
    } catch {
      // An unreadable clipboard is as unverified as a mismatched one.
    }
  }
  return false;
}

/**
 * Deliver `text` to the focused window (Linux).
 *
 * The clipboard already holds the text (`autoPasteText` wrote it before
 * calling here). Verify that with a read-back, then run
 * [linuxPastePlan]: synthesized Ctrl+V first when verified, typing as
 * the fallback.
 */
function linuxPaste(text: string): Promise<boolean> {
  // Run `tool` and resolve true when it exits successfully.
  const attempt = (file: string, args: string[]): Promise<boolean> =>
    new Promise((resolve) => {
      execFile(file, args, (err) => resolve(!err));
    });

  return (async () => {
    const verified = await clipboardHoldsText(text);
    const plan = linuxPastePlan(verified, !!process.env.WAYLAND_DISPLAY);
    for (const step of plan) {
      if (await attempt(step.tool, linuxPasteStepArgs(step, text))) return true;
    }
    console.warn("[autoPaste] Linux paste failed: no wtype, xdotool or ydotool available");
    return false;
  })();
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
 *
 * True everywhere now (canario-ubb): Linux has its toolchain plan
 * (probed per-delivery, with fallbacks), macOS/Windows route through
 * the sidecar's native backends. Per-dictation failures surface as a
 * `false` return with the text left on the clipboard.
 */
export function isAutoPasteAvailable(): boolean {
  return true;
}

// Hotkey health — types, parsing, and UI gating for the Linux evdev
// permission failure reported by the sidecar's `hotkey_status` command.
//
// The status is pull-based on purpose: the main process starts the
// hotkey listener (and its synchronous /dev/input access probe) before
// any renderer window exists, so a query issued on mount can't lose
// the failure to subscription timing.

/**
 * Wire shape of the sidecar's `hotkey_status` response data.
 * Mirrors `canario_core::HotkeyStatus` (all fields always present).
 */
export type HotkeyStatusInfo = {
  /** "evdev" | "x11" | "socket-fallback" | "not-started" */
  backend: string;
  /** True when /dev/input is unreadable (user not in the "input" group). */
  permission_denied: boolean;
  /** Copy-pasteable command that grants access; null unless permission_denied. */
  fix_command: string | null;
  /** Human-readable explanation of a degraded state, if any. */
  detail: string | null;
};

/** Parse a `hotkey_status` response payload; null if malformed. */
export function parseHotkeyStatus(data: unknown): HotkeyStatusInfo | null {
  if (!data || typeof data !== "object") return null;
  const d = data as Record<string, unknown>;
  if (typeof d.backend !== "string" || typeof d.permission_denied !== "boolean") return null;
  return {
    backend: d.backend,
    permission_denied: d.permission_denied,
    fix_command: typeof d.fix_command === "string" ? d.fix_command : null,
    detail: typeof d.detail === "string" ? d.detail : null,
  };
}

/**
 * Show the persistent permission guidance only when the hotkey is
 * genuinely blocked by `/dev/input` permissions on Linux.
 *
 * Deliberately NOT shown for:
 * - non-Linux platforms (macOS/Windows use Electron global shortcuts),
 * - "not-started" (hotkey startup failed or hasn't been attempted —
 *   nothing actionable about permissions),
 * - a socket fallback with another cause — no portal-based global-
 *   shortcut backend exists, so whenever permissions are the blocker
 *   the usermod fix is the actionable guidance. (If a working portal
 *   fallback is ever added, this gate must prefer it instead.)
 */
export function shouldShowHotkeyPermissionNotice(
  status: HotkeyStatusInfo | null | undefined,
  isLinux: boolean,
): boolean {
  if (!isLinux) return false;
  if (!status || !status.permission_denied) return false;
  return typeof status.fix_command === "string" && status.fix_command.length > 0;
}

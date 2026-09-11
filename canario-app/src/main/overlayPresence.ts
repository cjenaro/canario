// Overlay presence mode for the main process (canario-aud.2).
//
// Pure logic (no electron import) so it is node-testable, mirroring
// hotkeyRouting.ts / overlayStatus.ts. The main process reads the
// sidecar's AppConfig (its cache or a get_config payload) to decide:
//   "full" — show the overlay window, renderer paints the island
//   "dot"  — show the SAME overlay window, renderer paints only a dot
//   "tray" — never show the overlay window; the tray icon carries the
//            recording signal (updateTrayState already flips the tray
//            menu to "● Recording" / "⟳ Transcribing…" / "● Ready" from
//            the sidecar events — see onSidecarEvent in index.ts — so
//            tray mode needs no new plumbing there).
// Core's serde normalizes unknown values to "full" on load; this helper
// applies the same rule to whatever JSON the main process sees, so a
// stale cache or a hand-edited config never enables an unknown mode.

/** The indicator modes (mirrors core's AppConfig.overlay_presence). */
export type OverlayPresence = "full" | "dot" | "tray";

/**
 * Validate any config/UI value into an OverlayPresence. Unknown,
 * missing, or non-string values fall back to "full" — matching
 * core's normalize_overlay_presence (canario-aud.2).
 */
export function normalizeOverlayPresence(value: unknown): OverlayPresence {
  return value === "dot" || value === "tray" ? value : "full";
}

/**
 * Extract the presence mode from a config payload (the main-process
 * cache or get_config data). Missing/malformed reads as "full".
 */
export function overlayPresenceFromConfig(config: unknown): OverlayPresence {
  const cfg = (config ?? {}) as Record<string, unknown>;
  return normalizeOverlayPresence(cfg.overlay_presence);
}

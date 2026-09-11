// Overlay presence helpers (canario-aud.2) — the indicator style modes
// and the pure render-gating table that decides what the overlay page
// paints for each mode × lifecycle status. Pure logic (no DOM/Solid)
// so it can be unit-tested in node; OverlayPage.tsx owns the painting
// and main/overlayPresence.ts owns the main-process side (window
// visibility + the tray-mode suppression).
//
// Modes (AppConfig.overlay_presence, core normalizes unknown → "full"):
//   "full" — the recording island: pill, timer, live captions, and the
//            transcribing/transforming phases (the default)
//   "dot"  — a minimal pulsing dot near the island's position while
//            recording ONLY — no captions, no timer, no busy phases
//   "tray" — nothing on screen; the tray icon state carries the signal

import type { OverlayStatus } from "./overlayStatus";

/** The indicator modes (mirrors core's AppConfig.overlay_presence). */
export const OVERLAY_PRESENCE_MODES = ["full", "dot", "tray"] as const;
export type OverlayPresence = (typeof OVERLAY_PRESENCE_MODES)[number];

/**
 * Validate any config/UI value into an OverlayPresence. Unknown,
 * missing, or non-string values fall back to "full" — matching
 * core's normalize_overlay_presence (canario-aud.2).
 */
export function normalizeOverlayPresence(value: unknown): OverlayPresence {
  return value === "dot" || value === "tray" ? value : "full";
}

/**
 * Extract the presence mode from a get_config payload (AppConfig key:
 * `overlay_presence`). Missing/invalid values fall back to "full".
 */
export function overlayPresenceFromConfig(config: unknown): OverlayPresence {
  const cfg = (config ?? {}) as Record<string, unknown>;
  return normalizeOverlayPresence(cfg.overlay_presence);
}

/**
 * What the overlay page paints (canario-aud.2 render gating):
 *   island — the full recording island (pill/captions/busy label)
 *   dot    — the minimal recording dot
 * "tray" paints neither (the main process also keeps the window
 * hidden — this is the renderer-side half of that belt-and-braces).
 * The dot is recording-state only: busy phases and hidden paint
 * nothing, unlike the island's transcribing/transforming cards.
 */
export interface OverlayContent {
  island: boolean;
  dot: boolean;
}

export function overlayContentFor(
  mode: OverlayPresence,
  status: OverlayStatus,
): OverlayContent {
  switch (mode) {
    case "dot":
      return { island: false, dot: status === "recording" };
    case "tray":
      return { island: false, dot: false };
    default:
      // "full" — today's behavior exactly: the island for every
      // non-hidden status.
      return { island: status !== "hidden", dot: false };
  }
}

/**
 * The partial-config payload for an `update_config` command from the
 * settings UI (Settings → Appearance → Indicator). A single top-level
 * string key — nothing to merge wholesale.
 */
export function overlayPresenceConfigPayload(mode: OverlayPresence): {
  overlay_presence: OverlayPresence;
} {
  return { overlay_presence: normalizeOverlayPresence(mode) };
}

/** Rendered size of the minimal dot, px (placement math reuses it). */
export const DOT_SIZE = 10;

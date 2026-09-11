// Legacy theme.json parsing (canario-dmp.19).
//
// The theme mode used to be mirrored into a main-process
// userData/theme.json (written by the theme:set handler) so boots where
// the sidecar was unavailable still found the mode. AppConfig (the
// sidecar's config.json) is now the single source of truth, so index.ts
// imports the mirror's value once and deletes the file — mirroring the
// onboarding.json migration (canario-xv9, onboarding.ts). This module
// holds the pure piece so it stays unit-testable without Electron.

/** The AppConfig `theme` vocabulary (core's ThemeMode, lowercase serde). */
const THEME_MODES = ["dark", "light", "system"] as const;
export type ThemeMode = (typeof THEME_MODES)[number];

/**
 * Parse the contents of the legacy theme.json (`{"theme":"dark"}`).
 *
 * Returns the AppConfig ThemeMode to import, or null when there is
 * nothing to import (no file, corrupt JSON, or no `theme` key) — the
 * migration then just deletes the file. A `theme` value outside the
 * vocabulary maps to "dark", exactly what the pre-migration read
 * (`JSON.parse(...).theme` through the renderer's resolveThemeMode)
 * would have shown the user.
 */
export function parseLegacyThemeFile(raw: string | null): ThemeMode | null {
  if (raw === null) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  if (!("theme" in parsed)) return null;
  const theme = (parsed as { theme: unknown }).theme;
  return THEME_MODES.includes(theme as ThemeMode) ? (theme as ThemeMode) : "dark";
}

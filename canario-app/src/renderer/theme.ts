// Appearance application — set the persisted theme mode + accent on the
// document root.
//
// Persistence layout (see AppPage):
//   • AppConfig (sidecar) is the source of truth: `theme` + `accent_color`.
//   • localStorage caches the last applied appearance so index.html's
//     inline boot script can paint it before the app bundle runs (no
//     first-paint flash).
//   • theme.json (main process, unchanged ipc handlers) is mirrored on
//     every change so boots where the sidecar is unavailable still get
//     the user's theme mode.
//
// Pure validation/mapping lives in primitives/appearance.ts.

import {
  accentHoverColor,
  APPEARANCE_CACHE_KEY,
  parseAppearanceCache,
  resolveThemeMode,
  serializeAppearanceCache,
  type AccentColor,
  type Appearance,
  type ThemeMode,
} from "./primitives/appearance";

/**
 * Apply a theme mode + accent override to the document root.
 * The accent is written as inline `--accent` / `--accent-hover` custom
 * properties, overriding the per-theme palette from themes.css; `null`
 * removes the override so the theme defaults shine through again.
 */
export function applyAppearance(mode: ThemeMode, accent: AccentColor) {
  applyModeAttrs(mode);
  const root = document.documentElement;
  if (accent) {
    root.style.setProperty("--accent", accent);
    root.style.setProperty("--accent-hover", accentHoverColor(accent));
  } else {
    root.style.removeProperty("--accent");
    root.style.removeProperty("--accent-hover");
  }
}

/**
 * Apply a theme mode only ("dark" | "light" | "system"), leaving any
 * accent override untouched. Legacy mode-only entry point kept for
 * callers that don't know about accents (OnboardingPage).
 */
export function applyTheme(t: string) {
  applyModeAttrs(resolveThemeMode(t));
}

function applyModeAttrs(mode: ThemeMode) {
  const root = document.documentElement;
  if (mode === "light") {
    root.style.setProperty("color-scheme", "light");
    root.setAttribute("data-theme", "light");
  } else if (mode === "system") {
    root.style.removeProperty("color-scheme");
    root.removeAttribute("data-theme");
  } else {
    root.style.setProperty("color-scheme", "dark");
    root.setAttribute("data-theme", "dark");
  }
}

/**
 * Cache the authoritative appearance for the next boot's pre-paint
 * script (index.html). Best-effort: without it, the worst case is one
 * boot that paints the default theme before AppConfig arrives.
 */
export function cacheAppearanceForNextBoot(appearance: Appearance) {
  try {
    localStorage.setItem(APPEARANCE_CACHE_KEY, serializeAppearanceCache(appearance));
  } catch {
    // localStorage unavailable (or full) — ignore
  }
}

/**
 * Read the appearance cached by the last boot. Used to initialize the
 * page's signals so the first application matches what's already
 * painted; never authoritative.
 */
export function readCachedAppearance(): Appearance | null {
  try {
    const cached = parseAppearanceCache(localStorage.getItem(APPEARANCE_CACHE_KEY));
    if (!cached) return null;
    return { mode: cached.mode, accent: cached.accent };
  } catch {
    return null;
  }
}

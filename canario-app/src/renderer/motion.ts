// Animation gating application — resolve the stored preferences plus
// the OS reduced-motion request into data-* attributes on the
// document root, matched by styles/animations.css.
//
// Persistence layout (see AppPage):
//   • AppConfig (sidecar) is the source of truth: `animations`.
//   • localStorage caches the last applied settings so index.html's
//     inline boot script can gate pre-paint — before the app bundle
//     runs — and the overlay window re-applies cache writes live via
//     storage events.
//
// Pure resolution/mapping lives in primitives/animations.ts.

import {
  ANIMATIONS_CACHE_KEY,
  animationsRootAttributes,
  DEFAULT_ANIMATION_SETTINGS,
  parseAnimationsCache,
  serializeAnimationsCache,
  type AnimationSettings,
  type ResolvedAnimations,
} from "./primitives/animations";

/**
 * Apply resolved animation preferences to the document root. The
 * attributes gate the existing keyframes/transitions in
 * styles/animations.css — no per-component wiring needed.
 */
export function applyAnimations(resolved: ResolvedAnimations) {
  const root = document.documentElement;
  for (const [attr, value] of Object.entries(animationsRootAttributes(resolved))) {
    root.setAttribute(attr, value);
  }
}

/**
 * Live-track the OS `prefers-reduced-motion` request. Reports the
 * current value immediately and on every change; the resolver
 * force-disables animations while it is set, independent of the
 * stored toggle. Returns an unwatch function.
 */
export function watchPrefersReducedMotion(onChange: (reduced: boolean) => void): () => void {
  if (typeof window.matchMedia !== "function") {
    onChange(false);
    return () => {};
  }
  const query = window.matchMedia("(prefers-reduced-motion: reduce)");
  const report = () => onChange(query.matches);
  report();
  query.addEventListener("change", report);
  return () => query.removeEventListener("change", report);
}

/**
 * Cache the authoritative settings for the next boot's pre-paint
 * script (index.html) and the overlay window's live sync. Best-effort:
 * without it, the worst case is one boot whose first paint animates
 * before AppConfig arrives.
 */
export function cacheAnimationsForNextBoot(settings: AnimationSettings) {
  try {
    localStorage.setItem(ANIMATIONS_CACHE_KEY, serializeAnimationsCache(settings));
  } catch {
    // localStorage unavailable (or full) — ignore
  }
}

/**
 * Read the settings cached by the last boot. Used to initialize the
 * page's signals so the first application matches what's already
 * gated; never authoritative.
 */
export function readCachedAnimations(): AnimationSettings {
  try {
    return (
      parseAnimationsCache(localStorage.getItem(ANIMATIONS_CACHE_KEY)) ??
      { ...DEFAULT_ANIMATION_SETTINGS }
    );
  } catch {
    return { ...DEFAULT_ANIMATION_SETTINGS };
  }
}

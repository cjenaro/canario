// i18n infrastructure (canario-7ah.7 groundwork).
//
// Library: @solid-primitives/i18n — the standard Solid choice (stage-3
// primitive, ~1kB, reactive, no provider/context lock-in). We use its
// composable core (`translator` + `resolveTemplate`) with a module-scoped
// locale signal instead of the `createI18n` context provider: the app has
// three windows/pages plus non-component call sites (toast messages,
// createCanario error strings), and a provider would have to be threaded
// through all of them for no benefit while the locale is app-global.
//
// GROUNDWORK SCOPE — what exists vs. what's deliberately deferred:
//   Now: English-only catalog (en.ts), typed keys, navigator-based locale
//        resolution with "en" fallback, reactive t() usable anywhere.
//   Later (follow-up issues): a second locale catalog, a persisted user
//        choice (localStorage or AppConfig — the signal + setLocale below
//        are the seam), a language picker in Settings, and Electron
//        main-process strings (tray menu, dialogs — main-process Menu is
//        outside the renderer bundle; separate follow-up).
//
// Future locale wiring: add `xx.ts` typed as `Partial<EnglishCatalog>`,
// register it in `dictionaries` as `{ ...en, ...xx }` (per-key English
// fallback), widen `Locale`, and LOCALES — resolveLocale and t() already
// do the rest.

import { createSignal } from "solid-js";
import { resolveTemplate, translator } from "@solid-primitives/i18n";
import { en, type EnglishCatalog, type MessageKey } from "./en";

/** Locales with a shipped catalog. Widen as catalogs are added. */
export type Locale = "en";

/** Locale tags that resolve today (kept distinct from Locale for clarity). */
const AVAILABLE: readonly Locale[] = ["en"];

/** Placeholder structure for future catalogs — every locale falls back to en. */
const dictionaries: Record<Locale, EnglishCatalog> = { en };

/**
 * Resolve the locale from a browser preference list (navigator.languages):
 * walk the tags in order, accepting an exact match ("en") or a base-language
 * match ("en-GB" → "en"), case-insensitively. Unknown/empty → "en".
 */
export function resolveLocale(preferred: readonly string[] | undefined): Locale {
  if (!preferred) return "en";
  for (const tag of preferred) {
    if (typeof tag !== "string") continue;
    const normalized = tag.toLowerCase();
    const base = normalized.split("-")[0];
    for (const candidate of AVAILABLE) {
      if (normalized === candidate || base === candidate) return candidate;
    }
  }
  return "en";
}

// No stored user choice yet (groundwork): the locale is derived once at
// boot from the browser preference list. `setLocale` is the seam a future
// Settings picker + persisted choice will drive.
const [locale, setLocale] = createSignal<Locale>(
  resolveLocale(typeof navigator !== "undefined" ? navigator.languages : undefined),
);

/**
 * The typed, reactive translator.
 *   t("model.sectionTitle")                 → "Model"
 *   t("model.download", { name: "Parakeet TDT v3" }) → "Download Parakeet TDT v3"
 * Keys are `MessageKey` literals — typos fail typecheck. Reads the current
 * locale's dictionary on each call, so callers inside reactive scopes
 * (JSX, effects) re-render when the locale changes.
 */
export const t = translator(() => dictionaries[locale()], resolveTemplate);

export { locale, setLocale, en };
export type { EnglishCatalog, MessageKey };

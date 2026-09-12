// i18n infrastructure (canario-7ah.7 groundwork; persistence + second
// catalog landed in canario-tts).
//
// Library: @solid-primitives/i18n — the standard Solid choice (stage-3
// primitive, ~1kB, reactive, no provider/context lock-in). We use its
// composable core (`translator` + `resolveTemplate`) with a module-scoped
// locale signal instead of the `createI18n` context provider: the app has
// three windows/pages plus non-component call sites (toast messages,
// createCanario error strings), and a provider would have to be threaded
// through all of them for no benefit while the locale is app-global.
//
// Locale sources, in precedence order (canario-tts):
//   1. The persisted user choice — AppConfig.locale, written by the
//      Settings → Language picker and applied here via
//      applyConfigLocale() (boot + every ConfigChanged). The last
//      applied value is mirrored to localStorage so the very first
//      render of the next boot already matches (no language flash).
//   2. The OS/browser preference list (navigator.languages), for users
//      who never picked ("Automatic").
//
// Catalogs: en is the typed source of truth; every other locale is
// `Partial<EnglishCatalog>` merged over English (`{ ...en, ...xx }`),
// so an untranslated key falls back to English per-key.
//
// Still untranslated by design: sidecar/core Rust error strings (would
// need core-side locale plumbing) and index.html's pre-bundle /lang
// attribute.

import { createSignal } from "solid-js";
import { resolveTemplate, translator } from "@solid-primitives/i18n";
import { en, type Catalog, type EnglishCatalog, type MessageKey } from "./en";
import { es } from "./es";

/** Locales with a shipped catalog. Widen as catalogs are added. */
export type Locale = "en" | "es";

/** A persisted locale choice: "" (the default) = resolve automatically. */
export type LocaleChoice = "" | Locale;

/** Locale tags that resolve today (kept distinct from Locale for clarity). */
const AVAILABLE: readonly Locale[] = ["en", "es"];

/** Per-locale catalogs — every non-English locale falls back to en per-key. */
const dictionaries: Record<Locale, Catalog> = {
  en,
  es: { ...en, ...es },
};

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

/** localStorage mirror of the applied locale (next-boot pre-paint cache). */
const LOCALE_CACHE_KEY = "canario-locale";

function readCachedLocale(): Locale | null {
  try {
    const cached = localStorage.getItem(LOCALE_CACHE_KEY);
    return AVAILABLE.includes(cached as Locale) ? (cached as Locale) : null;
  } catch {
    return null; // storage unavailable (shouldn't happen in Electron)
  }
}

function cacheLocale(locale: Locale): void {
  try {
    localStorage.setItem(LOCALE_CACHE_KEY, locale);
  } catch {
    // Cache-only — the config remains the source of truth.
  }
}

// Boot order: cached explicit choice first (no flash), else the browser
// preference list. The config-driven applyConfigLocale() overrides both
// once the sidecar's config arrives (and on every ConfigChanged).
const [locale, setLocale] = createSignal<Locale>(
  readCachedLocale()
    ?? resolveLocale(typeof navigator !== "undefined" ? navigator.languages : undefined),
);

/**
 * Apply a persisted `locale` value from AppConfig. `""` (Automatic)
 * re-resolves from the browser preference list; an unknown value falls
 * back the same way (defensive against hand-edited configs). Idempotent
 * and cheap — call it on boot and on every ConfigChanged.
 */
export function applyConfigLocale(configLocale: unknown): void {
  const value = typeof configLocale === "string" ? configLocale : "";
  const next =
    value !== "" && AVAILABLE.includes(value as Locale)
      ? (value as Locale)
      : resolveLocale(typeof navigator !== "undefined" ? navigator.languages : undefined);
  setLocale(next);
  cacheLocale(next);
}

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

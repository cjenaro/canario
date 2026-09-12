// Main-process string catalog (canario-tts) — tray menu labels, tray
// tooltip, and the version-warning texts the tooltip carries.
//
// The renderer has its own richer i18n (renderer/i18n/); this file is
// deliberately separate because the main process is a different bundle
// (no solid-js, no renderer catalog import), the key set is tiny, and
// the renderer's dead-key completeness test must not see main-only
// keys as orphans.
//
// Locale source: AppConfig.locale, applied at boot (index.ts
// fetchConfig) and on every ConfigChanged — the same freshness posture
// as every other cached-config consumer. Unknown locale → English.

/** Locales with shipped main-process strings. */
export type MainLocale = "en" | "es";

/** Template params for the few interpolated messages. */
type Params = Record<string, string | number>;

const en = {
  "tray.tooltip.tagline": "Canario — Voice to Text",
  "tray.tooltip.offline": "⚠ backend offline — restart Canario",
  "tray.status.ready": "● Ready",
  "tray.status.recording": "● Recording",
  "tray.status.transcribing": "⟳ Transcribing…",
  "tray.toggle.start": "▶ Start Recording",
  "tray.toggle.stop": "■ Stop Recording",
  "tray.history": "📜 History",
  "tray.settings": "⚙ Settings",
  "tray.quit": "Quit",
  "version.protocolMismatch":
    "Protocol mismatch (app {{app}}, sidecar {{sidecar}}) — commands and events may have drifted; restart with a matching build",
  "version.staleSidecar":
    "Sidecar {{sidecar}} does not match app {{app}} — a stale backend may be running",
} as const;

const es: Partial<Record<keyof typeof en, string>> = {
  "tray.tooltip.tagline": "Canario — Voz a texto",
  "tray.tooltip.offline": "⚠ backend caído — reinicia Canario",
  "tray.status.ready": "● Listo",
  "tray.status.recording": "● Grabando",
  "tray.status.transcribing": "⟳ Transcribiendo…",
  "tray.toggle.start": "▶ Iniciar grabación",
  "tray.toggle.stop": "■ Detener grabación",
  "tray.history": "📜 Historial",
  "tray.settings": "⚙ Configuración",
  "tray.quit": "Salir",
  "version.protocolMismatch":
    "Incompatibilidad de protocolo (app {{app}}, sidecar {{sidecar}}) — los comandos y eventos pueden haber divergido; reinicia con una build que coincida",
  "version.staleSidecar":
    "El sidecar {{sidecar}} no coincide con la app {{app}} — puede haber un backend obsoleto corriendo",
};

const catalogs: Record<MainLocale, Record<string, string>> = {
  en: en as Record<string, string>,
  es: { ...en, ...es },
};

let currentLocale: MainLocale = "en";

/** Apply the persisted AppConfig.locale value ("" / unknown → en). */
export function setMainLocale(configLocale: unknown): void {
  const value = typeof configLocale === "string" ? configLocale : "";
  currentLocale = value === "es" || value === "en" ? value : "en";
}

/** The applied main-process locale (for diagnostics/tests). */
export function getMainLocale(): MainLocale {
  return currentLocale;
}

/**
 * Translate one main-process string, resolving `{{ param }}`
 * placeholders. A missing key (or a locale without it) falls back to
 * English, then to the raw key — a translation gap must never break
 * the tray.
 */
export function mainT(key: keyof typeof en, params?: Params): string {
  let template = catalogs[currentLocale][key] ?? en[key] ?? key;
  if (params) {
    for (const [name, value] of Object.entries(params)) {
      const v = String(value);
      // Accept both placeholder spellings (`{{name}}` / `{{ name }}`).
      template = template
        .split(`{{${name}}}`)
        .join(v)
        .split(`{{ ${name} }}`)
        .join(v);
    }
  }
  return template;
}

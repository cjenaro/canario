// Appearance helpers — theme mode + accent color validation and mapping.
// Pure logic (no DOM) so it can be unit-tested in node; theme.ts owns
// the DOM application and index.html owns the pre-paint boot script.

export const THEME_MODES = ["dark", "light", "system"] as const;
export type ThemeMode = (typeof THEME_MODES)[number];

/**
 * A custom accent color: a normalized 6-digit hex string ("#rrggbb"),
 * or null to use the per-theme default accent from themes.css.
 */
export type AccentColor = string | null;

export interface AccentPreset {
  /** Stable id (UI key) */
  id: string;
  /** Human-readable label */
  name: string;
  /** Normalized hex persisted to AppConfig.accent_color */
  hex: string;
}

/**
 * Swatches offered in Settings → Appearance. The "default" swatch
 * (accent = null) is rendered separately from these presets.
 */
export const ACCENT_PRESETS: readonly AccentPreset[] = [
  { id: "canary", name: "Canary", hex: "#e94560" },
  { id: "ocean", name: "Ocean", hex: "#3b82f6" },
  { id: "violet", name: "Violet", hex: "#8b5cf6" },
  { id: "emerald", name: "Emerald", hex: "#10b981" },
  { id: "amber", name: "Amber", hex: "#f59e0b" },
  { id: "rose", name: "Rose", hex: "#ec4899" },
] as const;

const HEX_RE = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/;

/** True for valid 3- or 6-digit hex colors ("#f53", "#e94560"). */
export function isHexColor(value: string): boolean {
  return HEX_RE.test(value.trim());
}

/** Normalize a hex color to lowercase 6-digit form; null when invalid. */
export function normalizeHexColor(value: string): string | null {
  const trimmed = value.trim();
  if (!HEX_RE.test(trimmed)) return null;
  if (trimmed.length === 4) {
    const [, r, g, b] = trimmed;
    return `#${r}${r}${g}${g}${b}${b}`.toLowerCase();
  }
  return trimmed.toLowerCase();
}

/** Validate any config/UI value into a ThemeMode (fallback: dark). */
export function resolveThemeMode(value: unknown): ThemeMode {
  return THEME_MODES.includes(value as ThemeMode) ? (value as ThemeMode) : "dark";
}

/** Validate any config/UI value into a normalized accent (null = default). */
export function resolveAccent(value: unknown): AccentColor {
  return typeof value === "string" ? normalizeHexColor(value) : null;
}

export interface Appearance {
  mode: ThemeMode;
  accent: AccentColor;
}

/**
 * Extract + validate the appearance from a get_config payload
 * (AppConfig keys: `theme`, `accent_color`). Missing/invalid values
 * fall back to the defaults (dark, per-theme accent).
 */
export function appearanceFromConfig(config: unknown): Appearance {
  const cfg = (config ?? {}) as Record<string, unknown>;
  return {
    mode: resolveThemeMode(cfg.theme),
    accent: resolveAccent(cfg.accent_color),
  };
}

/**
 * Hover variant for a custom accent: the accent mixed 20% toward white.
 * Mirrors the themes.css pairing --accent / --accent-hover (e.g. the
 * dark default #e94560 → #ff6b81 is ≈ this mix).
 */
export function accentHoverColor(hex: string): string {
  const normalized = normalizeHexColor(hex) ?? "#000000";
  const mix = (channel: number) => Math.round(channel + (255 - channel) * 0.2);
  const r = mix(parseInt(normalized.slice(1, 3), 16));
  const g = mix(parseInt(normalized.slice(3, 5), 16));
  const b = mix(parseInt(normalized.slice(5, 7), 16));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

// ── Pre-paint cache ────────────────────────────────────────────────────────
// localStorage payload read by the inline boot script in index.html so the
// persisted appearance is painted before the app bundle runs (no flash).
// `hover` is precomputed here so the boot script needs no color math.

export const APPEARANCE_CACHE_KEY = "canario.appearance";

export interface AppearanceCache {
  mode: ThemeMode;
  accent: AccentColor;
  hover: string | null;
}

export function serializeAppearanceCache(appearance: Appearance): string {
  const cache: AppearanceCache = {
    mode: appearance.mode,
    accent: appearance.accent,
    hover: appearance.accent ? accentHoverColor(appearance.accent) : null,
  };
  return JSON.stringify(cache);
}

/**
 * Parse a cached payload; null when missing or corrupt. Lenient by
 * design — the cache is a pre-paint hint, and AppConfig re-applies the
 * authoritative values right after boot.
 */
export function parseAppearanceCache(raw: string | null): AppearanceCache | null {
  if (typeof raw !== "string") return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  const obj = parsed as Record<string, unknown>;
  return {
    mode: resolveThemeMode(obj.mode),
    accent: resolveAccent(obj.accent),
    hover: resolveAccent(obj.hover),
  };
}

// Animation preferences — master toggle + per-effect toggles (PRD §8.4)
// + the OS prefers-reduced-motion override. Pure logic (no DOM) so it
// can be unit-tested in node; motion.ts owns the DOM application and
// index.html owns the pre-paint boot script (which duplicates this
// resolution inline — keep the two in sync).

/** The gated effects (PRD §8.4), keyed as AppConfig's `animations` block. */
export const ANIMATION_EFFECTS = [
  "overlay_slide",
  "recording_dot_pulse",
  "toggle_slide",
  "delete_slide",
  "window_fade",
] as const;
export type AnimationEffect = (typeof ANIMATION_EFFECTS)[number];

/** Wire shape of AppConfig.animations (snake_case, mirrored in core's AnimationSettings). */
export interface AnimationSettings {
  /** Master switch — off kills every effect. */
  enabled: boolean;
  /** Recording overlay slide-down + fade on appear. */
  overlay_slide: boolean;
  /** Pulsing red recording dot. */
  recording_dot_pulse: boolean;
  /** Toggle-switch slide + color change. */
  toggle_slide: boolean;
  /** History-item slide-left + fade on delete. */
  delete_slide: boolean;
  /** Window-open fade + scale. */
  window_fade: boolean;
}

/** Existing behavior: everything on. */
export const DEFAULT_ANIMATION_SETTINGS: AnimationSettings = {
  enabled: true,
  overlay_slide: true,
  recording_dot_pulse: true,
  toggle_slide: true,
  delete_slide: true,
  window_fade: true,
};

/** Validate one flag: only an explicit `false` disables the effect. */
function resolveFlag(value: unknown): boolean {
  return value !== false;
}

/**
 * Extract + validate the animation preferences from a get_config
 * payload (AppConfig key: `animations`). Lenient per flag — anything
 * other than an explicit `false` (including a missing or non-object
 * block) keeps the effect on, so corrupt/stale data never silently
 * disables motion the user didn't turn off.
 */
export function animationsFromConfig(config: unknown): AnimationSettings {
  const cfg = (config ?? {}) as Record<string, unknown>;
  const raw = cfg.animations;
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    return { ...DEFAULT_ANIMATION_SETTINGS };
  }
  const block = raw as Record<string, unknown>;
  return {
    enabled: resolveFlag(block.enabled),
    overlay_slide: resolveFlag(block.overlay_slide),
    recording_dot_pulse: resolveFlag(block.recording_dot_pulse),
    toggle_slide: resolveFlag(block.toggle_slide),
    delete_slide: resolveFlag(block.delete_slide),
    window_fade: resolveFlag(block.window_fade),
  };
}

/** Effective state after combining the stored prefs with the OS request. */
export interface ResolvedAnimations {
  /** Master state after the reduced-motion override. */
  enabled: boolean;
  /** True while the OS prefers-reduced-motion request is active. */
  reducedMotion: boolean;
  /** Per-effect verdict: master on AND that effect's flag on. */
  effects: Record<AnimationEffect, boolean>;
}

/**
 * Resolve the stored preferences against the OS `prefers-reduced-motion`
 * request. Reduced motion force-disables everything, independent of the
 * stored toggles — which survive untouched for when the OS setting goes
 * away again.
 */
export function resolveAnimations(
  settings: AnimationSettings,
  prefersReducedMotion: boolean,
): ResolvedAnimations {
  const enabled = settings.enabled && !prefersReducedMotion;
  const effects = Object.fromEntries(
    ANIMATION_EFFECTS.map((effect) => [effect, enabled && settings[effect]]),
  ) as Record<AnimationEffect, boolean>;
  return { enabled, reducedMotion: prefersReducedMotion, effects };
}

/**
 * Root attribute name gating one effect
 * (e.g. `overlay_slide` → `data-anim-overlay-slide`).
 */
export function animationEffectAttribute(effect: AnimationEffect): string {
  return `data-anim-${effect.replace(/_/g, "-")}`;
}

/**
 * The document-root attributes encoding a resolved state — what
 * motion.ts applies and styles/animations.css matches:
 *
 *   data-animations="off"       master off (or reduced motion)
 *   data-anim-<effect>="off"    that one effect disabled
 *
 * "on" values are inert; the CSS selectors only match "off".
 */
export function animationsRootAttributes(
  resolved: ResolvedAnimations,
): Record<string, "on" | "off"> {
  const attrs: Record<string, "on" | "off"> = {
    "data-animations": resolved.enabled ? "on" : "off",
  };
  for (const effect of ANIMATION_EFFECTS) {
    attrs[animationEffectAttribute(effect)] = resolved.effects[effect] ? "on" : "off";
  }
  return attrs;
}

// ── Persistence ────────────────────────────────────────────────────────────

/**
 * update_config payload for a settings change. The sidecar's merge
 * replaces top-level keys wholesale, so the FULL block must travel
 * with every update — a partial block would reset unmentioned flags
 * to their defaults (see core's animations_apply_as_a_whole_key test).
 */
export function animationsConfigPayload(
  settings: AnimationSettings,
): { animations: AnimationSettings } {
  return { animations: { ...settings } };
}

// ── Pre-paint cache ────────────────────────────────────────────────────────
// localStorage payload read by the inline boot script in index.html so the
// gating attributes are set before the app bundle runs (no animated first
// paint), and re-applied live in the overlay window via storage events.
// Mirrors the appearance cache (APPEARANCE_CACHE_KEY).

export const ANIMATIONS_CACHE_KEY = "canario.animations";

export function serializeAnimationsCache(settings: AnimationSettings): string {
  return JSON.stringify(settings);
}

/**
 * Parse a cached payload; null when missing or corrupt. Lenient by
 * design — the cache is a pre-paint hint, AppConfig re-applies the
 * authoritative values right after boot.
 */
export function parseAnimationsCache(raw: string | null): AnimationSettings | null {
  if (typeof raw !== "string") return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  // Reuse the lenient config parsing on the block itself.
  return animationsFromConfig({ animations: parsed });
}

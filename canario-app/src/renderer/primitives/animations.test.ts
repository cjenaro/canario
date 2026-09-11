// Tests for the animation preference helpers — config parsing, toggle
// resolution (incl. the reduced-motion override), root-attribute
// mapping, the wholesale update payload, and the pre-paint cache.
import { describe, it, expect } from "vitest";
import {
  ANIMATION_EFFECTS,
  animationEffectAttribute,
  animationsConfigPayload,
  animationsFromConfig,
  animationsRootAttributes,
  ANIMATIONS_CACHE_KEY,
  DEFAULT_ANIMATION_SETTINGS,
  parseAnimationsCache,
  resolveAnimations,
  serializeAnimationsCache,
  type AnimationSettings,
} from "./animations";

const ALL_ON: AnimationSettings = { ...DEFAULT_ANIMATION_SETTINGS };

const MIXED: AnimationSettings = {
  enabled: true,
  overlay_slide: true,
  recording_dot_pulse: false,
  toggle_slide: false,
  delete_slide: true,
  window_fade: false,
};

describe("ANIMATION_EFFECTS / DEFAULT_ANIMATION_SETTINGS", () => {
  it("defaults to everything on (existing behavior)", () => {
    expect(ALL_ON.enabled).toBe(true);
    for (const effect of ANIMATION_EFFECTS) {
      expect(ALL_ON[effect]).toBe(true);
    }
  });

  it("matches the serde field names of core's AnimationSettings", () => {
    // Keep in sync with canario-core config/mod.rs — these are the
    // AppConfig.animations wire keys.
    expect(ANIMATION_EFFECTS).toEqual([
      "overlay_slide",
      "recording_dot_pulse",
      "toggle_slide",
      "delete_slide",
      "window_fade",
    ]);
    expect(Object.keys(DEFAULT_ANIMATION_SETTINGS)).toEqual([
      "enabled",
      ...ANIMATION_EFFECTS,
    ]);
  });
});

describe("animationsFromConfig", () => {
  it("reads the AppConfig animations block", () => {
    expect(
      animationsFromConfig({
        animations: {
          enabled: false,
          overlay_slide: true,
          recording_dot_pulse: false,
          toggle_slide: true,
          delete_slide: false,
          window_fade: true,
        },
      }),
    ).toEqual({
      enabled: false,
      overlay_slide: true,
      recording_dot_pulse: false,
      toggle_slide: true,
      delete_slide: false,
      window_fade: true,
    });
  });

  it("falls back to all-on when the block is absent or not an object", () => {
    expect(animationsFromConfig({})).toEqual(ALL_ON);
    expect(animationsFromConfig(null)).toEqual(ALL_ON);
    expect(animationsFromConfig(undefined)).toEqual(ALL_ON);
    expect(animationsFromConfig({ animations: null })).toEqual(ALL_ON);
    expect(animationsFromConfig({ animations: "nope" })).toEqual(ALL_ON);
    expect(animationsFromConfig({ animations: [] })).toEqual(ALL_ON);
    expect(animationsFromConfig({ animations: true })).toEqual(ALL_ON);
  });

  it("is lenient per flag — only an explicit false disables", () => {
    expect(animationsFromConfig({ animations: { enabled: false } })).toEqual({
      ...ALL_ON,
      enabled: false,
    });
    expect(
      animationsFromConfig({ animations: { recording_dot_pulse: false } }),
    ).toEqual({ ...ALL_ON, recording_dot_pulse: false });
    // Wrong types keep the effect on (corrupt data never silently
    // disables motion the user didn't turn off)
    expect(
      animationsFromConfig({ animations: { enabled: "yes", overlay_slide: 0 } }),
    ).toEqual(ALL_ON);
  });

  it("ignores unrelated config keys", () => {
    expect(animationsFromConfig({ model: "ParakeetV3", animations: { enabled: false } })).toEqual({
      ...ALL_ON,
      enabled: false,
    });
  });
});

describe("resolveAnimations", () => {
  it("keeps everything on when the prefs are on and motion is fine", () => {
    const resolved = resolveAnimations(ALL_ON, false);
    expect(resolved.enabled).toBe(true);
    expect(resolved.reducedMotion).toBe(false);
    for (const effect of ANIMATION_EFFECTS) {
      expect(resolved.effects[effect]).toBe(true);
    }
  });

  it("master off kills every effect, whatever the flags say", () => {
    const resolved = resolveAnimations({ ...MIXED, enabled: false }, false);
    expect(resolved.enabled).toBe(false);
    for (const effect of ANIMATION_EFFECTS) {
      expect(resolved.effects[effect]).toBe(false);
    }
  });

  it("per-effect flags gate only their own effect while the master is on", () => {
    const resolved = resolveAnimations(MIXED, false);
    expect(resolved.enabled).toBe(true);
    expect(resolved.effects.overlay_slide).toBe(true);
    expect(resolved.effects.recording_dot_pulse).toBe(false);
    expect(resolved.effects.toggle_slide).toBe(false);
    expect(resolved.effects.delete_slide).toBe(true);
    expect(resolved.effects.window_fade).toBe(false);
  });

  it("reduced motion force-disables everything — even with every toggle on", () => {
    const resolved = resolveAnimations(ALL_ON, true);
    expect(resolved.enabled).toBe(false);
    expect(resolved.reducedMotion).toBe(true);
    for (const effect of ANIMATION_EFFECTS) {
      expect(resolved.effects[effect]).toBe(false);
    }
  });

  it("reduced motion wins over a partially-off config too", () => {
    const resolved = resolveAnimations(MIXED, true);
    expect(resolved.enabled).toBe(false);
    expect(resolved.effects.overlay_slide).toBe(false);
    expect(resolved.effects.delete_slide).toBe(false);
  });
});

describe("animationsRootAttributes", () => {
  it("maps the master state to data-animations", () => {
    expect(animationsRootAttributes(resolveAnimations(ALL_ON, false))["data-animations"]).toBe("on");
    expect(animationsRootAttributes(resolveAnimations(ALL_ON, true))["data-animations"]).toBe("off");
    expect(
      animationsRootAttributes(resolveAnimations({ ...ALL_ON, enabled: false }, false))[
        "data-animations"
      ],
    ).toBe("off");
  });

  it("marks each disabled effect with its own attribute", () => {
    const attrs = animationsRootAttributes(resolveAnimations(MIXED, false));
    expect(attrs["data-anim-overlay-slide"]).toBe("on");
    expect(attrs["data-anim-recording-dot-pulse"]).toBe("off");
    expect(attrs["data-anim-toggle-slide"]).toBe("off");
    expect(attrs["data-anim-delete-slide"]).toBe("on");
    expect(attrs["data-anim-window-fade"]).toBe("off");
  });

  it("reduced motion turns every attribute off", () => {
    const attrs = animationsRootAttributes(resolveAnimations(ALL_ON, true));
    expect(Object.values(attrs).every((v) => v === "off")).toBe(true);
  });

  it("attribute names are kebab-case data-anim-<effect>", () => {
    expect(animationEffectAttribute("overlay_slide")).toBe("data-anim-overlay-slide");
    expect(animationEffectAttribute("recording_dot_pulse")).toBe("data-anim-recording-dot-pulse");
    expect(animationEffectAttribute("toggle_slide")).toBe("data-anim-toggle-slide");
    expect(animationEffectAttribute("delete_slide")).toBe("data-anim-delete-slide");
    expect(animationEffectAttribute("window_fade")).toBe("data-anim-window-fade");
  });

  it("always emits the full attribute set (no stale values survive)", () => {
    const attrs = animationsRootAttributes(resolveAnimations(ALL_ON, false));
    expect(Object.keys(attrs).sort()).toEqual(
      [
        "data-animations",
        ...ANIMATION_EFFECTS.map((effect) => animationEffectAttribute(effect)),
      ].sort(),
    );
  });
});

describe("animationsConfigPayload", () => {
  it("wraps the settings under the animations key", () => {
    expect(animationsConfigPayload(MIXED)).toEqual({ animations: MIXED });
  });

  it("carries every flag — update_config merges top-level keys wholesale", () => {
    // A partial block would reset unmentioned flags to their defaults
    // (see core's animations_apply_as_a_whole_key test), so the payload
    // must always include the full block.
    const payload = animationsConfigPayload({ ...ALL_ON, enabled: false });
    expect(Object.keys(payload.animations).sort()).toEqual(
      ["enabled", ...ANIMATION_EFFECTS].sort(),
    );
  });

  it("returns a copy — callers can't mutate the caller's settings", () => {
    const settings = { ...ALL_ON };
    const payload = animationsConfigPayload(settings);
    payload.animations.enabled = false;
    expect(settings.enabled).toBe(true);
  });
});

describe("pre-paint cache", () => {
  it("uses the canario.animations key", () => {
    expect(ANIMATIONS_CACHE_KEY).toBe("canario.animations");
  });

  it("round-trips through serialize/parse", () => {
    for (const settings of [ALL_ON, MIXED, { ...ALL_ON, enabled: false }]) {
      expect(parseAnimationsCache(serializeAnimationsCache(settings))).toEqual(settings);
    }
  });

  it("returns null for missing or corrupt payloads", () => {
    expect(parseAnimationsCache(null)).toBeNull();
    expect(parseAnimationsCache("")).toBeNull();
    expect(parseAnimationsCache("not json")).toBeNull();
    expect(parseAnimationsCache("42")).toBeNull();
    expect(parseAnimationsCache('"a string"')).toBeNull();
    expect(parseAnimationsCache("null")).toBeNull();
    expect(parseAnimationsCache("[]")).toBeNull();
  });

  it("is lenient with invalid values inside the payload", () => {
    // Same rule as the config parsing: only an explicit false disables
    expect(parseAnimationsCache('{"enabled":false,"overlay_slide":true}')).toEqual({
      ...ALL_ON,
      enabled: false,
    });
    expect(parseAnimationsCache('{"bogus":"x"}')).toEqual(ALL_ON);
  });
});

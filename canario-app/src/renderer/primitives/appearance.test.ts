// Tests for the appearance helpers (theme mode resolution + accent
// validation/normalization + pre-paint cache round trip)
import { describe, it, expect } from "vitest";
import {
  ACCENT_PRESETS,
  accentHoverColor,
  appearanceFromConfig,
  isHexColor,
  normalizeHexColor,
  parseAppearanceCache,
  resolveAccent,
  resolveThemeMode,
  serializeAppearanceCache,
  THEME_MODES,
  type Appearance,
} from "./appearance";

describe("resolveThemeMode", () => {
  it("passes the three valid modes through", () => {
    expect(resolveThemeMode("dark")).toBe("dark");
    expect(resolveThemeMode("light")).toBe("light");
    expect(resolveThemeMode("system")).toBe("system");
  });

  it("falls back to dark for anything else", () => {
    expect(resolveThemeMode("Dark")).toBe("dark"); // case-sensitive
    expect(resolveThemeMode("blue")).toBe("dark");
    expect(resolveThemeMode("")).toBe("dark");
    expect(resolveThemeMode(undefined)).toBe("dark");
    expect(resolveThemeMode(null)).toBe("dark");
    expect(resolveThemeMode(42)).toBe("dark");
    expect(resolveThemeMode({ mode: "light" })).toBe("dark");
  });
});

describe("isHexColor", () => {
  it("accepts 6-digit hex with or without a hash-prefix casing", () => {
    expect(isHexColor("#e94560")).toBe(true);
    expect(isHexColor("#E94560")).toBe(true);
    expect(isHexColor("#000000")).toBe(true);
    expect(isHexColor("#ffffff")).toBe(true);
  });

  it("accepts 3-digit hex shorthand", () => {
    expect(isHexColor("#f53")).toBe(true);
    expect(isHexColor("#F53")).toBe(true);
  });

  it("tolerates surrounding whitespace", () => {
    expect(isHexColor("  #e94560  ")).toBe(true);
  });

  it("rejects everything else", () => {
    expect(isHexColor("e94560")).toBe(false); // missing #
    expect(isHexColor("#e9456")).toBe(false); // 5 digits
    expect(isHexColor("#e945600")).toBe(false); // 7 digits
    expect(isHexColor("#f5")).toBe(false); // 2 digits
    expect(isHexColor("#")).toBe(false);
    expect(isHexColor("#zzzzzz")).toBe(false); // not hex digits
    expect(isHexColor("red")).toBe(false);
    expect(isHexColor("")).toBe(false);
    expect(isHexColor("   ")).toBe(false);
    expect(isHexColor("rgb(1, 2, 3)")).toBe(false);
  });
});

describe("normalizeHexColor", () => {
  it("lowercases 6-digit hex", () => {
    expect(normalizeHexColor("#E94560")).toBe("#e94560");
  });

  it("expands 3-digit shorthand", () => {
    expect(normalizeHexColor("#F53")).toBe("#ff5533");
    expect(normalizeHexColor("#abc")).toBe("#aabbcc");
  });

  it("trims whitespace first", () => {
    expect(normalizeHexColor("  #3B82F6 ")).toBe("#3b82f6");
  });

  it("returns null for invalid input", () => {
    expect(normalizeHexColor("nope")).toBeNull();
    expect(normalizeHexColor("3b82f6")).toBeNull();
    expect(normalizeHexColor("#3b82f")).toBeNull();
    expect(normalizeHexColor("")).toBeNull();
  });
});

describe("resolveAccent", () => {
  it("normalizes valid hex strings", () => {
    expect(resolveAccent("#e94560")).toBe("#e94560");
    expect(resolveAccent("#3B82F6")).toBe("#3b82f6");
    expect(resolveAccent(" #f53 ")).toBe("#ff5533");
  });

  it("maps invalid or non-string values to null (theme default)", () => {
    expect(resolveAccent("hot pink")).toBeNull();
    expect(resolveAccent("")).toBeNull();
    expect(resolveAccent(null)).toBeNull();
    expect(resolveAccent(undefined)).toBeNull();
    expect(resolveAccent(42)).toBeNull();
  });
});

describe("appearanceFromConfig", () => {
  it("reads the AppConfig keys (theme, accent_color)", () => {
    expect(appearanceFromConfig({ theme: "light", accent_color: "#3B82F6" })).toEqual({
      mode: "light",
      accent: "#3b82f6",
    });
    expect(appearanceFromConfig({ theme: "system", accent_color: null })).toEqual({
      mode: "system",
      accent: null,
    });
  });

  it("falls back to dark + default accent when absent or invalid", () => {
    expect(appearanceFromConfig({})).toEqual({ mode: "dark", accent: null });
    expect(appearanceFromConfig(null)).toEqual({ mode: "dark", accent: null });
    expect(appearanceFromConfig(undefined)).toEqual({ mode: "dark", accent: null });
    expect(appearanceFromConfig({ theme: "solarized", accent_color: "blue" })).toEqual({
      mode: "dark",
      accent: null,
    });
  });

  it("ignores unrelated config keys", () => {
    expect(appearanceFromConfig({ model: "ParakeetV3", theme: "light" })).toEqual({
      mode: "light",
      accent: null,
    });
  });
});

describe("accentHoverColor", () => {
  it("mixes 20% toward white", () => {
    // 233,69,96 → 237,106,128
    expect(accentHoverColor("#e94560")).toBe("#ed6a80");
    // 0,0,0 → 51,51,51
    expect(accentHoverColor("#000000")).toBe("#333333");
    // 59,130,246 → 98,155,248
    expect(accentHoverColor("#3b82f6")).toBe("#629bf8");
  });

  it("saturates at white", () => {
    expect(accentHoverColor("#ffffff")).toBe("#ffffff");
  });

  it("accepts 3-digit shorthand (normalized first)", () => {
    // #F53 → #ff5533 → 255,119,92
    expect(accentHoverColor("#F53")).toBe("#ff775c");
  });

  it("falls back to black for invalid input", () => {
    expect(accentHoverColor("not-a-color")).toBe("#333333");
  });
});

describe("pre-paint cache", () => {
  it("serializes with a precomputed hover variant", () => {
    expect(serializeAppearanceCache({ mode: "light", accent: "#000000" })).toBe(
      JSON.stringify({ mode: "light", accent: "#000000", hover: "#333333" }),
    );
    expect(serializeAppearanceCache({ mode: "dark", accent: null })).toBe(
      JSON.stringify({ mode: "dark", accent: null, hover: null }),
    );
  });

  it("round-trips through parseAppearanceCache", () => {
    const appearances: Appearance[] = [
      { mode: "dark", accent: null },
      { mode: "system", accent: "#3b82f6" },
      { mode: "light", accent: "#e94560" },
    ];
    for (const appearance of appearances) {
      expect(parseAppearanceCache(serializeAppearanceCache(appearance))).toEqual({
        ...appearance,
        hover: appearance.accent ? accentHoverColor(appearance.accent) : null,
      });
    }
  });

  it("returns null for missing or corrupt payloads", () => {
    expect(parseAppearanceCache(null)).toBeNull();
    expect(parseAppearanceCache("")).toBeNull();
    expect(parseAppearanceCache("not json")).toBeNull();
    expect(parseAppearanceCache("42")).toBeNull();
    expect(parseAppearanceCache('"a string"')).toBeNull();
    expect(parseAppearanceCache("null")).toBeNull();
  });

  it("is lenient with invalid values inside the payload", () => {
    expect(parseAppearanceCache('{"mode":"bogus"}')).toEqual({
      mode: "dark",
      accent: null,
      hover: null,
    });
    expect(parseAppearanceCache('{"mode":"light","accent":"#E94560"}')).toEqual({
      mode: "light",
      accent: "#e94560",
      hover: null,
    });
    // Scalars/arrays are rejected wholesale
    expect(parseAppearanceCache("[]")).toBeNull();
  });
});

describe("ACCENT_PRESETS", () => {
  it("only contains normalized 6-digit hex values", () => {
    for (const preset of ACCENT_PRESETS) {
      expect(normalizeHexColor(preset.hex)).toBe(preset.hex);
    }
  });

  it("has unique hex values and stable ids", () => {
    const hexes = new Set(ACCENT_PRESETS.map((p) => p.hex));
    const ids = new Set(ACCENT_PRESETS.map((p) => p.id));
    expect(hexes.size).toBe(ACCENT_PRESETS.length);
    expect(ids.size).toBe(ACCENT_PRESETS.length);
  });
});

describe("THEME_MODES", () => {
  it("matches the serde names of core's ThemeMode enum", () => {
    expect(THEME_MODES).toEqual(["dark", "light", "system"]);
  });
});

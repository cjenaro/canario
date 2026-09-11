// parseLegacyThemeFile — pure parsing for the one-time theme.json →
// AppConfig migration (canario-dmp.19). The fs/sidecar wiring lives in
// index.ts's migrateLegacyTheme (Electron-bound, not unit-testable
// without mocking app/sendCommand beyond reason — same split as
// onboarding.ts).
import { describe, expect, it } from "vitest";
import { parseLegacyThemeFile } from "./legacyTheme";

describe("parseLegacyThemeFile", () => {
  it("reads each mode the mirror could store", () => {
    expect(parseLegacyThemeFile('{"theme":"dark"}')).toBe("dark");
    expect(parseLegacyThemeFile('{"theme":"light"}')).toBe("light");
    expect(parseLegacyThemeFile('{"theme":"system"}')).toBe("system");
  });

  it("maps off-vocabulary values to dark, like the legacy read did", () => {
    // The pre-migration theme:get returned the raw string and the
    // renderer's resolveThemeMode coerced junk to "dark" — the
    // migration must preserve that observable behavior.
    expect(parseLegacyThemeFile('{"theme":"blue"}')).toBe("dark");
    expect(parseLegacyThemeFile('{"theme":123}')).toBe("dark");
  });

  it("null when there is nothing to import", () => {
    expect(parseLegacyThemeFile(null)).toBeNull(); // no legacy file
    expect(parseLegacyThemeFile('{"accent":"#ffffff"}')).toBeNull(); // no theme key
    expect(parseLegacyThemeFile("{}")).toBeNull();
    expect(parseLegacyThemeFile('["dark"]')).toBeNull(); // never a theme.json shape
    expect(parseLegacyThemeFile("{ this is not json")).toBeNull();
    expect(parseLegacyThemeFile("")).toBeNull();
  });
});

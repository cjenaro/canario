// Main-process string catalog tests (canario-tts): locale switching,
// English fallback, and placeholder substitution for the tray +
// version-warning strings.
import { afterEach, describe, expect, it } from "vitest";
import { getMainLocale, mainT, setMainLocale } from "./strings";

describe("main string catalog", () => {
  afterEach(() => {
    setMainLocale(""); // back to the default (English)
  });

  it("defaults to English", () => {
    expect(getMainLocale()).toBe("en");
    expect(mainT("tray.quit")).toBe("Quit");
    expect(mainT("tray.status.recording")).toBe("● Recording");
  });

  it("switches locales from the persisted config value", () => {
    setMainLocale("es");
    expect(getMainLocale()).toBe("es");
    expect(mainT("tray.quit")).toBe("Salir");
    expect(mainT("tray.status.recording")).toBe("● Grabando");
    expect(mainT("tray.tooltip.tagline")).toBe("Canario — Voz a texto");
  });

  it("falls back to English on unknown values", () => {
    setMainLocale("xx-pirate");
    expect(getMainLocale()).toBe("en");
    expect(mainT("tray.settings")).toBe("⚙ Settings");
  });

  it("substitutes placeholders in both spellings", () => {
    setMainLocale("en");
    expect(mainT("version.staleSidecar", { sidecar: "0.1.1", app: "0.1.2" })).toBe(
      "Sidecar 0.1.1 does not match app 0.1.2 — a stale backend may be running",
    );
    setMainLocale("es");
    expect(mainT("version.staleSidecar", { sidecar: "0.1.1", app: "0.1.2" })).toBe(
      "El sidecar 0.1.1 no coincide con la app 0.1.2 — puede haber un backend obsoleto corriendo",
    );
  });
});

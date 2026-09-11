// Tests for the main-process overlay presence mode (canario-aud.2):
// the config value decides whether the overlay window may show at all
// ("tray" suppresses it) — missing, malformed, or unknown values read
// as "full", mirroring core's normalize_overlay_presence.
import { describe, it, expect } from "vitest";
import { normalizeOverlayPresence, overlayPresenceFromConfig } from "./overlayPresence";

describe("normalizeOverlayPresence", () => {
  it("passes the three valid modes through", () => {
    expect(normalizeOverlayPresence("full")).toBe("full");
    expect(normalizeOverlayPresence("dot")).toBe("dot");
    expect(normalizeOverlayPresence("tray")).toBe("tray");
  });

  it.each([
    ["case-sensitive", "Dot"],
    ["typo", "banana"],
    ["empty", ""],
    ["undefined", undefined],
    ["null", null],
    ["number", 3],
    ["object", { mode: "dot" }],
  ])("falls back to full for %s", (_label, value) => {
    expect(normalizeOverlayPresence(value)).toBe("full");
  });
});

describe("overlayPresenceFromConfig", () => {
  it("reads the mode from a config payload", () => {
    expect(overlayPresenceFromConfig({ overlay_presence: "dot" })).toBe("dot");
    expect(overlayPresenceFromConfig({ overlay_presence: "tray" })).toBe("tray");
    expect(overlayPresenceFromConfig({ overlay_presence: "full" })).toBe("full");
  });

  it.each([
    ["null config", null],
    ["undefined config", undefined],
    ["no key (old configs)", { auto_paste: true }],
    ["unknown value (hand-edited)", { overlay_presence: "minimal" }],
    ["non-string value", { overlay_presence: 2 }],
  ])("reads %s as full", (_label, config) => {
    expect(overlayPresenceFromConfig(config)).toBe("full");
  });
});

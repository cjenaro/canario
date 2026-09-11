// Tests for the overlay presence helpers (canario-aud.2): mode
// normalization (unknown → full), config extraction, the pure
// mode × status render-gating table, and the settings payload.
import { describe, it, expect } from "vitest";
import type { OverlayStatus } from "./overlayStatus";
import {
  DOT_SIZE,
  normalizeOverlayPresence,
  overlayContentFor,
  overlayPresenceConfigPayload,
  overlayPresenceFromConfig,
  OVERLAY_PRESENCE_MODES,
  type OverlayPresence,
} from "./overlayPresence";

const STATUSES: OverlayStatus[] = ["hidden", "recording", "transcribing", "transforming"];

describe("normalizeOverlayPresence", () => {
  it("passes the three valid modes through", () => {
    expect(normalizeOverlayPresence("full")).toBe("full");
    expect(normalizeOverlayPresence("dot")).toBe("dot");
    expect(normalizeOverlayPresence("tray")).toBe("tray");
  });

  it.each([
    ["case-sensitive", "Tray"],
    ["typo", "minimal"],
    ["empty", ""],
    ["undefined", undefined],
    ["null", null],
    ["number", 1],
  ])("falls back to full for %s", (_label, value) => {
    expect(normalizeOverlayPresence(value)).toBe("full");
  });
});

describe("overlayPresenceFromConfig", () => {
  it("reads the mode from a get_config payload", () => {
    expect(overlayPresenceFromConfig({ overlay_presence: "dot" })).toBe("dot");
    expect(overlayPresenceFromConfig({ overlay_presence: "tray" })).toBe("tray");
    expect(overlayPresenceFromConfig({ overlay_presence: "full" })).toBe("full");
  });

  it.each([
    ["null config", null],
    ["undefined config", undefined],
    ["no key (old configs)", { live_captions: true }],
    ["unknown value", { overlay_presence: "none" }],
    ["non-string value", { overlay_presence: true }],
  ])("reads %s as full", (_label, config) => {
    expect(overlayPresenceFromConfig(config)).toBe("full");
  });
});

describe("overlayContentFor", () => {
  it("full mode keeps today's behavior: the island for every non-hidden status", () => {
    for (const status of STATUSES) {
      expect(overlayContentFor("full", status)).toEqual({
        island: status !== "hidden",
        dot: false,
      });
    }
  });

  it("dot mode paints the dot only while recording", () => {
    expect(overlayContentFor("dot", "recording")).toEqual({ island: false, dot: true });
    // No busy phases, no hidden-state ghost: recording-state only.
    expect(overlayContentFor("dot", "hidden")).toEqual({ island: false, dot: false });
    expect(overlayContentFor("dot", "transcribing")).toEqual({ island: false, dot: false });
    expect(overlayContentFor("dot", "transforming")).toEqual({ island: false, dot: false });
  });

  it("tray mode paints nothing for every status", () => {
    for (const status of STATUSES) {
      expect(overlayContentFor("tray", status)).toEqual({ island: false, dot: false });
    }
  });

  it("covers every mode × status combination without falling through", () => {
    for (const mode of OVERLAY_PRESENCE_MODES) {
      for (const status of STATUSES) {
        const content = overlayContentFor(mode, status);
        // The two indicators are mutually exclusive…
        expect(content.island && content.dot).toBe(false);
        // …and tray mode never shows either (belt and braces with the
        // main-side window suppression).
        if (mode === "tray") {
          expect(content.island).toBe(false);
          expect(content.dot).toBe(false);
        }
      }
    }
  });
});

describe("overlayPresenceConfigPayload", () => {
  it("wraps the mode as the update_config key", () => {
    expect(overlayPresenceConfigPayload("tray")).toEqual({ overlay_presence: "tray" });
    expect(overlayPresenceConfigPayload("dot")).toEqual({ overlay_presence: "dot" });
    expect(overlayPresenceConfigPayload("full")).toEqual({ overlay_presence: "full" });
  });

  it("normalizes before persisting — an invalid value can never be written", () => {
    expect(overlayPresenceConfigPayload("banana" as OverlayPresence)).toEqual({
      overlay_presence: "full",
    });
  });
});

describe("DOT_SIZE", () => {
  it("is a small fixed size used for both the render and the clamp math", () => {
    expect(DOT_SIZE).toBeGreaterThan(4);
    expect(DOT_SIZE).toBeLessThan(24);
  });
});

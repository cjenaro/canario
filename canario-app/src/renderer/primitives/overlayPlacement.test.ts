// Tests for the overlay placement helpers (per-monitor drag offsets:
// default/clamp/drag math + AppConfig serialization round trips)
import { describe, it, expect } from "vitest";
import {
  OVERLAY_DEFAULT_TOP,
  OVERLAY_EDGE_MARGIN,
  applyDragDelta,
  clampOverlayPosition,
  defaultOverlayPosition,
  normalizeOverlayOffset,
  overlayOffsetsConfigPayload,
  overlayOffsetsFromConfig,
  resolveOverlayPosition,
  toStorableOffset,
  withOverlayOffset,
  withoutOverlayOffset,
  type OverlayOffsets,
} from "./overlayPlacement";

describe("defaultOverlayPosition", () => {
  it("centers the island at the top of the monitor", () => {
    expect(defaultOverlayPosition(1920, 120)).toEqual({ x: 900, y: OVERLAY_DEFAULT_TOP });
    expect(defaultOverlayPosition(1280, 560)).toEqual({ x: 360, y: 12 });
  });

  it("matches the pre-drag pt-3 top gap", () => {
    expect(OVERLAY_DEFAULT_TOP).toBe(12); // 0.75rem
  });

  it("centers exactly on odd viewport/width combos (fractional x is fine live)", () => {
    expect(defaultOverlayPosition(1001, 120)).toEqual({ x: 440.5, y: 12 });
  });
});

describe("resolveOverlayPosition", () => {
  it("uses the stored offset when the monitor has one", () => {
    expect(resolveOverlayPosition({ x: 640, y: 480 }, 1920, 120)).toEqual({ x: 640, y: 480 });
  });

  it("falls back to the default top-center placement", () => {
    const fallback = resolveOverlayPosition(null, 1920, 120);
    expect(fallback).toEqual(defaultOverlayPosition(1920, 120));
    expect(resolveOverlayPosition(undefined, 1920, 120)).toEqual(fallback);
  });
});

describe("clampOverlayPosition", () => {
  const viewport = { width: 1920, height: 1080 };

  it("passes through positions that already fit", () => {
    expect(clampOverlayPosition({ x: 900, y: 12 }, viewport, { width: 120, height: 28 })).toEqual({
      x: 900,
      y: 12,
    });
  });

  it("enforces the edge margin on every side", () => {
    const clamped = clampOverlayPosition({ x: -50, y: -3 }, viewport, { width: 120, height: 28 });
    expect(clamped).toEqual({ x: OVERLAY_EDGE_MARGIN, y: OVERLAY_EDGE_MARGIN });
    // Bottom-right corner: max x = 1920 - 120 - 8, max y = 1080 - 28 - 8
    expect(
      clampOverlayPosition({ x: 5000, y: 5000 }, viewport, { width: 120, height: 28 }),
    ).toEqual({ x: 1792, y: 1044 });
  });

  it("re-clamps a stored offset for a monitor that shrank since the drag", () => {
    // Offset stored on a 1920-wide monitor, now shown on 1280x720.
    expect(
      clampOverlayPosition({ x: 1500, y: 1000 }, { width: 1280, height: 720 }, {
        width: 120,
        height: 28,
      }),
    ).toEqual({ x: 1152, y: 684 });
  });

  it("centers an island too big for the viewport instead of pinning it to an edge", () => {
    // 560px caption card on a 400px-wide display: centered at -80.
    expect(clampOverlayPosition({ x: 8, y: 8 }, { width: 400, height: 300 }, {
      width: 560,
      height: 100,
    })).toEqual({ x: -80, y: 8 });
    expect(clampOverlayPosition({ x: 390, y: 250 }, { width: 400, height: 300 }, {
      width: 560,
      height: 320,
    })).toEqual({ x: -80, y: -10 });
  });

  it("honors a custom margin", () => {
    expect(
      clampOverlayPosition({ x: 0, y: 0 }, viewport, { width: 100, height: 20 }, 40),
    ).toEqual({ x: 40, y: 40 });
  });
});

describe("applyDragDelta", () => {
  it("moves the start position by the pointer travel", () => {
    expect(applyDragDelta({ x: 100, y: 50 }, 240, -30)).toEqual({ x: 340, y: 20 });
    expect(applyDragDelta({ x: 100, y: 50 }, -1000, 0)).toEqual({ x: -900, y: 50 });
  });
});

describe("normalizeOverlayOffset", () => {
  it("rounds finite numbers to integers (core's OverlayOffset is i32)", () => {
    expect(normalizeOverlayOffset({ x: 100.4, y: 49.6 })).toEqual({ x: 100, y: 50 });
    expect(normalizeOverlayOffset({ x: -0.5, y: 0 })).toEqual({ x: 0, y: 0 });
  });

  it("accepts negative integers (offsets can legitimately be negative)", () => {
    expect(normalizeOverlayOffset({ x: -20, y: -5 })).toEqual({ x: -20, y: -5 });
  });

  it("rejects non-objects and non-finite members", () => {
    expect(normalizeOverlayOffset(null)).toBeNull();
    expect(normalizeOverlayOffset(42)).toBeNull();
    expect(normalizeOverlayOffset("100,200")).toBeNull();
    expect(normalizeOverlayOffset({ x: "100", y: 200 })).toBeNull();
    expect(normalizeOverlayOffset({ x: 100 })).toBeNull();
    expect(normalizeOverlayOffset({ x: Number.NaN, y: 0 })).toBeNull();
    expect(normalizeOverlayOffset({ x: Number.POSITIVE_INFINITY, y: 0 })).toBeNull();
  });
});

describe("overlayOffsetsFromConfig", () => {
  it("reads the AppConfig overlay_offsets map", () => {
    const config = {
      overlay_offsets: {
        "2305843009213693953": { x: 640, y: 12 },
        "7": { x: -20, y: 900 },
      },
    };
    expect(overlayOffsetsFromConfig(config)).toEqual({
      "2305843009213693953": { x: 640, y: 12 },
      "7": { x: -20, y: 900 },
    });
  });

  it("drops invalid entries instead of failing the whole map", () => {
    expect(
      overlayOffsetsFromConfig({
        overlay_offsets: {
          "1": { x: 10.6, y: 20 }, // fractional → rounded
          "2": { x: "nope", y: 20 }, // invalid → dropped
          "3": null, // missing → dropped
        },
      }),
    ).toEqual({ "1": { x: 11, y: 20 } });
  });

  it("returns an empty map for absent/malformed config values", () => {
    expect(overlayOffsetsFromConfig({})).toEqual({});
    expect(overlayOffsetsFromConfig(null)).toEqual({});
    expect(overlayOffsetsFromConfig(undefined)).toEqual({});
    expect(overlayOffsetsFromConfig({ overlay_offsets: null })).toEqual({});
    expect(overlayOffsetsFromConfig({ overlay_offsets: "nope" })).toEqual({});
    expect(overlayOffsetsFromConfig({ overlay_offsets: [{ x: 1, y: 2 }] })).toEqual({});
  });

  it("ignores unrelated config keys", () => {
    expect(overlayOffsetsFromConfig({ model: "ParakeetV3", overlay_offsets: { "1": { x: 0, y: 0 } } })).toEqual({
      "1": { x: 0, y: 0 },
    });
  });
});

describe("map helpers", () => {
  const offsets: OverlayOffsets = {
    "1": { x: 10, y: 20 },
    "2": { x: 300, y: 400 },
  };

  it("withOverlayOffset sets one monitor, keeping the others", () => {
    expect(withOverlayOffset(offsets, "3", { x: 5, y: 6 })).toEqual({
      "1": { x: 10, y: 20 },
      "2": { x: 300, y: 400 },
      "3": { x: 5, y: 6 },
    });
    expect(withOverlayOffset(offsets, "1", { x: 11, y: 22 })).toEqual({
      "1": { x: 11, y: 22 },
      "2": { x: 300, y: 400 },
    });
    // The input map is not mutated
    expect(offsets["1"]).toEqual({ x: 10, y: 20 });
  });

  it("withoutOverlayOffset removes one monitor, keeping the others", () => {
    expect(withoutOverlayOffset(offsets, "1")).toEqual({ "2": { x: 300, y: 400 } });
    expect(withoutOverlayOffset(offsets, "404")).toEqual(offsets);
    expect(withoutOverlayOffset({}, "1")).toEqual({});
  });
});

describe("toStorableOffset", () => {
  it("rounds a live dragged position", () => {
    expect(toStorableOffset({ x: 812.37, y: 143.5 })).toEqual({ x: 812, y: 144 });
  });
});

describe("overlayOffsetsConfigPayload", () => {
  it("wraps the full map in the update_config key", () => {
    expect(overlayOffsetsConfigPayload({ "1": { x: 10, y: 20 } })).toEqual({
      overlay_offsets: { "1": { x: 10, y: 20 } },
    });
  });

  it("serializes an empty map for reset-to-default (update_config cannot delete keys)", () => {
    expect(overlayOffsetsConfigPayload({})).toEqual({ overlay_offsets: {} });
  });
});

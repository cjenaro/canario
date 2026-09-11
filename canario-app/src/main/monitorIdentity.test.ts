// Tests for the frontend-agnostic monitor identity (canario-dmp.21):
// key scheme, collision tie-breaks, and the legacy id-key re-keying.
import { describe, it, expect } from "vitest";
import {
  legacyMonitorRefs,
  migrateLegacyOverlayOffsetKeys,
  monitorKey,
  resolveMonitorKeys,
  type MonitorSource,
} from "./monitorIdentity";

/** Minimal Display-like helper. */
function display(overrides: Partial<MonitorSource> & { id: number }): MonitorSource {
  return {
    label: "",
    bounds: { x: 0, y: 0, width: 1920, height: 1080 },
    scaleFactor: 1,
    ...overrides,
  };
}

describe("monitorKey", () => {
  it("uses the platform label as-is when non-empty", () => {
    expect(monitorKey(display({ id: 1, label: "DP-1" }))).toBe("DP-1");
    expect(monitorKey(display({ id: 1, label: "eDP-1", bounds: { x: 0, y: 0, width: 2560, height: 1600 } }))).toBe("eDP-1");
  });

  it("falls back to the bounds composite when the label is empty", () => {
    expect(monitorKey(display({ id: 1 }))).toBe("1920x1080@0,0");
    expect(monitorKey(display({ id: 1, bounds: { x: 1920, y: 0, width: 1280, height: 720 } }))).toBe("1280x720@1920,0");
  });

  it("treats whitespace-only labels as empty", () => {
    expect(monitorKey(display({ id: 1, label: "   " }))).toBe("1920x1080@0,0");
  });

  it("degrades the composite gracefully for missing/invalid bounds fields", () => {
    expect(monitorKey({ id: 1 })).toBe("0x0@0,0");
    expect(
      monitorKey({ id: 1, bounds: { x: Number.NaN, y: 10, width: 800, height: Number.POSITIVE_INFINITY } }),
    ).toBe("800x0@0,10");
  });

  it("never contains the Chromium display id", () => {
    // The old scheme keyed by Display.id.toString() — the new keys must
    // not reduce to that integer string.
    expect(monitorKey(display({ id: 2305843009213693953, label: "HDMI-A-1" }))).not.toBe("2305843009213693953");
  });
});

describe("resolveMonitorKeys", () => {
  it("passes distinct labels through untouched", () => {
    const keys = resolveMonitorKeys([
      display({ id: 1, label: "eDP-1" }),
      display({ id: 2, label: "DP-1", bounds: { x: 1920, y: 0, width: 1920, height: 1080 } }),
    ]);
    expect(keys).toEqual(["eDP-1", "DP-1"]);
  });

  it("disambiguates mirrored composite collisions with @scaleFactor", () => {
    // Two label-less displays mirroring each other: same bounds,
    // different scale factors.
    const keys = resolveMonitorKeys([
      display({ id: 1, scaleFactor: 1 }),
      display({ id: 2, scaleFactor: 2 }),
    ]);
    expect(keys).toEqual(["1920x1080@0,0@1", "1920x1080@0,0@2"]);
  });

  it("falls back to #2, #3… when bounds AND scale factor are identical", () => {
    const keys = resolveMonitorKeys([display({ id: 1 }), display({ id: 2 }), display({ id: 3 })]);
    expect(keys).toEqual(["1920x1080@0,0@1", "1920x1080@0,0@1#2", "1920x1080@0,0@1#3"]);
  });

  it("never produces duplicate keys, and never shadows an unambiguous base", () => {
    // A display literally labeled like another's would-be suffix.
    const keys = resolveMonitorKeys([
      display({ id: 1, label: "DP-1" }),
      display({ id: 2, label: "DP-1" }),
      display({ id: 3, label: "DP-1@1" }),
    ]);
    expect(new Set(keys).size).toBe(3);
    expect(keys).toContain("DP-1@1"); // the pre-existing label stays
    expect(keys.filter((k) => k === "DP-1@1")).toHaveLength(1);
  });

  it("resolves an empty display set", () => {
    expect(resolveMonitorKeys([])).toEqual([]);
  });
});

describe("legacyMonitorRefs", () => {
  it("zips legacy id keys with resolved identity keys", () => {
    expect(
      legacyMonitorRefs([
        display({ id: 7, label: "DP-1" }),
        display({ id: 2, bounds: { x: 1920, y: 0, width: 1280, height: 720 } }),
      ]),
    ).toEqual([
      { legacyId: "7", key: "DP-1" },
      { legacyId: "2", key: "1280x720@1920,0" },
    ]);
  });

  it("skips displays without a finite id", () => {
    expect(legacyMonitorRefs([{ label: "DP-1" }])).toEqual([]);
  });
});

describe("migrateLegacyOverlayOffsetKeys", () => {
  const connected = [
    { legacyId: "1", key: "DP-1" },
    { legacyId: "2", key: "eDP-1" },
  ];

  it("moves id-keyed offsets to identity keys and drops the old keys", () => {
    const res = migrateLegacyOverlayOffsetKeys(
      { "1": { x: 10, y: 20 }, "2": { x: 300, y: 400 } },
      connected,
    );
    expect(res?.next).toEqual({ "DP-1": { x: 10, y: 20 }, "eDP-1": { x: 300, y: 400 } });
    expect(res?.moved).toEqual([
      { from: "1", to: "DP-1" },
      { from: "2", to: "eDP-1" },
    ]);
  });

  it("keeps identity-keyed values when both schemes have one, dropping the legacy key", () => {
    const res = migrateLegacyOverlayOffsetKeys(
      { "1": { x: 10, y: 20 }, "DP-1": { x: 99, y: 99 } },
      connected,
    );
    expect(res?.next).toEqual({ "DP-1": { x: 99, y: 99 } });
    expect(res?.moved).toEqual([]); // nothing re-keyed, just a stale key dropped
  });

  it("leaves keys that match no connected display alone", () => {
    // Unplugged monitor's numeric key: stays. Note it can never match
    // again under the new scheme — accepted (re-plug re-defaults).
    const res = migrateLegacyOverlayOffsetKeys(
      { "42": { x: 5, y: 6 }, "some-weird-key": { x: 7, y: 8 } },
      connected,
    );
    expect(res).toBeNull();
  });

  it("migrates only the connected subset", () => {
    const res = migrateLegacyOverlayOffsetKeys(
      { "2": { x: 1, y: 2 }, "42": { x: 5, y: 6 } },
      connected,
    );
    expect(res?.next).toEqual({ "eDP-1": { x: 1, y: 2 }, "42": { x: 5, y: 6 } });
    expect(res?.moved).toEqual([{ from: "2", to: "eDP-1" }]);
  });

  it("returns null for empty/absent/malformed maps (no update_config round-trip)", () => {
    expect(migrateLegacyOverlayOffsetKeys({}, connected)).toBeNull();
    expect(migrateLegacyOverlayOffsetKeys(null, connected)).toBeNull();
    expect(migrateLegacyOverlayOffsetKeys(undefined, connected)).toBeNull();
    expect(migrateLegacyOverlayOffsetKeys([{ x: 1, y: 2 }], connected)).toBeNull();
    expect(migrateLegacyOverlayOffsetKeys("nope", connected)).toBeNull();
  });

  it("does not mutate the input map", () => {
    const offsets = { "1": { x: 10, y: 20 } };
    migrateLegacyOverlayOffsetKeys(offsets, connected);
    expect(offsets).toEqual({ "1": { x: 10, y: 20 } });
  });

  it("is independent of the connected-displays order", () => {
    const offsets = { "1": { x: 10, y: 20 }, "2": { x: 300, y: 400 } };
    const a = migrateLegacyOverlayOffsetKeys(offsets, connected);
    const b = migrateLegacyOverlayOffsetKeys(offsets, [...connected].reverse());
    expect(a?.next).toEqual(b?.next);
  });
});

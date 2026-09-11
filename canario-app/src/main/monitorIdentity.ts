// Monitor identity (canario-dmp.21).
//
// config.json's `overlay_offsets: BTreeMap<String, OverlayOffset>` keys
// the user's drag-to-reposition overlay placement per monitor
// (canario-aud.1). The keys used to be Electron `Display.id.toString()`
// — Chromium-internal integers that mean nothing to the GTK frontend
// and are not stable identifiers across sessions in principle. This
// module owns the replacement identity plus the one-time re-key
// migration, as pure functions (no Electron import) so they stay
// unit-testable in node — same split as onboarding.ts.
//
// Identity scheme (frozen decision, canario-dmp.21):
//   1. `Display.label` when non-empty — on Linux the xrandr output name
//      ("DP-1", "eDP-1"): stable, human-meaningful, and derivable by the
//      GTK frontend too. Used as-is.
//   2. Fallback composite `<width>x<height>@<x>,<y>` from Display.bounds
//      when the label is empty (headless/odd platforms).

/** Structural slice of Electron's `Display` the identity depends on. */
export interface MonitorSource {
  id?: number;
  label?: string;
  bounds?: { x?: number; y?: number; width?: number; height?: number };
  scaleFactor?: number;
}

/** One `{ x, y }` placement — core's `OverlayOffset`, structurally. */
export interface OverlayOffsetValue {
  x: number;
  y: number;
}

/** The persisted per-monitor placements map (AppConfig `overlay_offsets`). */
export type OverlayOffsetsMap = Record<string, OverlayOffsetValue>;

function finiteOr(value: number | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/**
 * Frontend-agnostic monitor identity for one display:
 * the platform label when present, else a deterministic composite of
 * the bounds. Labels are unique per connected output in practice; see
 * resolveMonitorKeys for the (rare) collision tie-break.
 */
export function monitorKey(display: MonitorSource): string {
  const label = (display.label ?? "").trim();
  if (label) return label;
  const b = display.bounds;
  const width = finiteOr(b?.width, 0);
  const height = finiteOr(b?.height, 0);
  const x = finiteOr(b?.x, 0);
  const y = finiteOr(b?.y, 0);
  return `${width}x${height}@${x},${y}`;
}

/**
 * Collision-resolved identities for a set of connected displays,
 * aligned to the input order.
 *
 * Tie-break: when two displays produce the same base key (mirrored
 * bounds under the composite fallback — labels are unique per output in
 * practice), append `@<scaleFactor>`; if that still collides (identical
 * bounds AND scale factor), append `#2`, `#3`, … in enumeration order
 * until unused. Unambiguous base keys are reserved first so a suffix
 * can never shadow another monitor's key.
 *
 * The `#n` fallback is NOT stable across replug ordering — accepted:
 * placement is a cosmetic preference and the worst case is one of the
 * colliding monitors snapping back to the default top-center position.
 */
export function resolveMonitorKeys(displays: readonly MonitorSource[]): string[] {
  const bases = displays.map(monitorKey);
  const baseCounts = new Map<string, number>();
  for (const base of bases) {
    baseCounts.set(base, (baseCounts.get(base) ?? 0) + 1);
  }

  const used = new Set<string>();
  // Reserve every unambiguous base first.
  for (const base of bases) {
    if ((baseCounts.get(base) ?? 0) === 1) used.add(base);
  }

  return bases.map((base, i) => {
    if ((baseCounts.get(base) ?? 0) === 1) return base;
    const scale = finiteOr(displays[i]?.scaleFactor, 1);
    const withScale = `${base}@${scale}`;
    if (!used.has(withScale)) {
      used.add(withScale);
      return withScale;
    }
    let n = 2;
    while (used.has(`${withScale}#${n}`)) n += 1;
    const key = `${withScale}#${n}`;
    used.add(key);
    return key;
  });
}

/** A connected display's old (pre-dmp.21) and new placement keys. */
export interface LegacyMonitorRef {
  /** `Display.id.toString()` — the key scheme this replaces. */
  legacyId: string;
  /** The frontend-agnostic identity (resolveMonitorKeys output). */
  key: string;
}

/**
 * Zip a set of connected displays into their legacy/new key pairs —
 * the input to migrateLegacyOverlayOffsetKeys.
 */
export function legacyMonitorRefs(displays: readonly MonitorSource[]): LegacyMonitorRef[] {
  const keys = resolveMonitorKeys(displays);
  const refs: LegacyMonitorRef[] = [];
  displays.forEach((display, i) => {
    if (typeof display.id === "number" && Number.isFinite(display.id)) {
      refs.push({ legacyId: String(display.id), key: keys[i] });
    }
  });
  return refs;
}

/** Result of a re-key migration that changed something. */
export interface OverlayOffsetsMigration {
  /** Full replacement map (update_config merges top-level keys wholesale). */
  next: OverlayOffsetsMap;
  /** Legacy keys that were re-keyed, for logging. */
  moved: Array<{ from: string; to: string }>;
}

/**
 * One-time re-key of id-keyed placements to identity keys
 * (canario-dmp.21), for the CURRENTLY connected displays:
 *
 * - a legacy key equal to a connected `Display.id.toString()` whose
 *   identity key is absent → its offset is copied to the identity key;
 * - the legacy key is then DROPPED (the map is internal; nothing else
 *   reads it);
 * - if both keys exist, the identity-keyed value wins and the legacy
 *   key is dropped;
 * - keys matching no connected display (numeric keys of an unplugged
 *   monitor, or unrelated junk) are left untouched. Note an unplugged
 *   monitor's legacy key can never match again under the new scheme —
 *   it just re-defaults to top-center when replugged. Accepted.
 *
 * Reads only from a snapshot of `offsets`, so the outcome is
 * independent of the connected-displays order. Returns null when
 * nothing needs changing, so the caller can skip the update_config
 * round-trip.
 */
export function migrateLegacyOverlayOffsetKeys(
  offsets: unknown,
  connected: readonly LegacyMonitorRef[],
): OverlayOffsetsMigration | null {
  if (typeof offsets !== "object" || offsets === null || Array.isArray(offsets)) return null;
  const snapshot = offsets as OverlayOffsetsMap;

  const droppedLegacy = new Set<string>();
  const assignments: Array<{ key: string; value: OverlayOffsetValue }> = [];
  const moved: Array<{ from: string; to: string }> = [];
  const assignedKeys = new Set<string>();

  for (const { legacyId, key } of connected) {
    if (!(legacyId in snapshot)) continue;
    droppedLegacy.add(legacyId);
    // New-scheme value already stored for this monitor — it wins.
    if (key in snapshot || assignedKeys.has(key)) continue;
    assignments.push({ key, value: snapshot[legacyId] });
    assignedKeys.add(key);
    moved.push({ from: legacyId, to: key });
  }

  if (droppedLegacy.size === 0) return null;

  const next: OverlayOffsetsMap = {};
  for (const [key, value] of Object.entries(snapshot)) {
    if (!droppedLegacy.has(key)) next[key] = value;
  }
  for (const { key, value } of assignments) {
    next[key] = value;
  }
  return { next, moved };
}

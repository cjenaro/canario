// Overlay placement helpers — drag offset math + AppConfig serialization
// for the recording island's per-monitor position (canario-aud.1).
// Pure logic (no DOM/Electron) so it can be unit-tested in node;
// OverlayPage.tsx owns the pointer handling and IPC wiring.

/** Island placement for one monitor, relative to that monitor's origin. */
export interface OverlayOffset {
  x: number;
  y: number;
}

/**
 * Per-monitor placements, keyed by Electron `Display.id` (as a string —
 * JSON object keys are strings). Mirrors core's
 * `AppConfig.overlay_offsets: BTreeMap<String, OverlayOffset>`.
 */
export type OverlayOffsets = Record<string, OverlayOffset>;

/** Viewport (= the overlay window, which exactly covers one monitor). */
export interface Viewport {
  width: number;
  height: number;
}

/** The island's current outer size (px). */
export interface IslandSize {
  width: number;
  height: number;
}

/** Top gap of the default placement (matches the old `pt-3` = 0.75rem). */
export const OVERLAY_DEFAULT_TOP = 12;

/** Minimum gap kept between the island and each viewport edge (px). */
export const OVERLAY_EDGE_MARGIN = 8;

/**
 * Default placement: horizontally centered at the top of the monitor —
 * what the island did before dragging existed (CSS `justify-center` +
 * `pt-3`). Reactive to the island width so the caption-card morph keeps
 * a default-positioned island centered.
 */
export function defaultOverlayPosition(viewportWidth: number, islandWidth: number): OverlayOffset {
  return { x: (viewportWidth - islandWidth) / 2, y: OVERLAY_DEFAULT_TOP };
}

/**
 * Position for this render: the stored offset when the monitor has one,
 * else the default top-center. The caller clamps the result against the
 * current viewport/island size (monitor may have changed since the drag).
 */
export function resolveOverlayPosition(
  stored: OverlayOffset | null | undefined,
  viewportWidth: number,
  islandWidth: number,
): OverlayOffset {
  return stored ?? defaultOverlayPosition(viewportWidth, islandWidth);
}

/**
 * Keep the island fully inside the viewport with a margin on every side.
 * An island too big for the viewport (e.g. the 560px caption card on a
 * tiny display) centers on that axis instead of hugging an edge.
 */
export function clampOverlayPosition(
  pos: OverlayOffset,
  viewport: Viewport,
  island: IslandSize,
  margin: number = OVERLAY_EDGE_MARGIN,
): OverlayOffset {
  const clampAxis = (p: number, viewportSize: number, islandSize: number): number => {
    const max = viewportSize - islandSize - margin;
    if (max < margin) return (viewportSize - islandSize) / 2; // too big: center
    return Math.min(max, Math.max(margin, p));
  };
  return {
    x: clampAxis(pos.x, viewport.width, island.width),
    y: clampAxis(pos.y, viewport.height, island.height),
  };
}

/** One drag step: move the start position by the pointer's travel. */
export function applyDragDelta(
  start: OverlayOffset,
  dx: number,
  dy: number,
): OverlayOffset {
  return { x: start.x + dx, y: start.y + dy };
}

// ── AppConfig serialization ────────────────────────────────────────────────

/**
 * Validate one raw config entry into a storable offset: finite numbers,
 * rounded to integers (core's `OverlayOffset` is i32 DIPs; client coords
 * are fractional). Invalid entries map to null and are dropped.
 */
export function normalizeOverlayOffset(value: unknown): OverlayOffset | null {
  if (typeof value !== "object" || value === null) return null;
  const { x, y } = value as Record<string, unknown>;
  if (typeof x !== "number" || !Number.isFinite(x)) return null;
  if (typeof y !== "number" || !Number.isFinite(y)) return null;
  // `+ 0` normalizes -0 (Math.round(-0.5)) so offsets stay clean integers
  return { x: Math.round(x) + 0, y: Math.round(y) + 0 };
}

/**
 * Extract + validate the per-monitor placements from a get_config
 * payload (AppConfig key: `overlay_offsets`). Invalid entries are
 * dropped rather than fatal — placement is a cosmetic preference.
 */
export function overlayOffsetsFromConfig(config: unknown): OverlayOffsets {
  const cfg = (config ?? {}) as Record<string, unknown>;
  const raw = cfg.overlay_offsets;
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
  const offsets: OverlayOffsets = {};
  for (const [displayId, value] of Object.entries(raw)) {
    const offset = normalizeOverlayOffset(value);
    if (offset) offsets[displayId] = offset;
  }
  return offsets;
}

/**
 * Round a live (dragged) position into storable form — same validation
 * and rounding as config entries, so what is persisted matches what
 * core's serde expects (integers).
 */
export function toStorableOffset(pos: OverlayOffset): OverlayOffset {
  const normalized = normalizeOverlayOffset(pos);
  return normalized ?? { x: 0, y: 0 };
}

/** Copy the map with one monitor's placement set (others untouched). */
export function withOverlayOffset(
  offsets: OverlayOffsets,
  displayId: string,
  offset: OverlayOffset,
): OverlayOffsets {
  return { ...offsets, [displayId]: offset };
}

/** Copy the map with one monitor's placement removed (reset). */
export function withoutOverlayOffset(
  offsets: OverlayOffsets,
  displayId: string,
): OverlayOffsets {
  const next: OverlayOffsets = {};
  for (const [id, offset] of Object.entries(offsets)) {
    if (id !== displayId) next[id] = offset;
  }
  return next;
}

/**
 * The partial-config payload for an `update_config` command. update_config
 * merges TOP-LEVEL keys wholesale, so the FULL map must be sent every
 * time — otherwise other monitors' placements would be clobbered.
 */
export function overlayOffsetsConfigPayload(offsets: OverlayOffsets): {
  overlay_offsets: OverlayOffsets;
} {
  return { overlay_offsets: { ...offsets } };
}

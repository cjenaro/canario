// History-entry presentation logic (canario-fgm.4) — the "transformed"
// badge shown on AppPage history rows. Pure so it is node-testable
// (mirrors primitives/animations.ts).
//
// fgm.1 D3: history.text is the CANONICAL value — what the user
// received (the transformed text when a transformation ran); the
// sidecar additionally stores raw_text ONLY when it differs from text
// (no transform → nothing stored, no storage doubling). Entries that
// predate the feature lack the field entirely — read it defensively.

/** The fields the badge decision needs from a get_history entry. */
export interface HistoryEntryLike {
  /** Canonical text (the transformed value when a transformation ran). */
  text: string;
  /** Raw pre-transform transcript — optional; absent on old data. */
  raw_text?: unknown;
}

/**
 * Did this entry go through a (successful) transformation? True only
 * when a non-empty raw_text is stored that differs from the canonical
 * text; absent, empty, equal, or non-string values all read as "no".
 */
export function isTransformedHistoryEntry(entry: HistoryEntryLike): boolean {
  const raw = entry.raw_text;
  return typeof raw === "string" && raw.length > 0 && raw !== entry.text;
}

// Tests for the history "transformed" badge decision (fgm.4): the
// badge marks entries whose stored raw_text differs from the canonical
// text (fgm.1 D3). The field is absent on old data and whenever no
// transformation ran, so every shape but a differing non-empty raw
// reads as "no badge".
import { describe, it, expect } from "vitest";
import { isTransformedHistoryEntry } from "./historyEntry";

describe("isTransformedHistoryEntry", () => {
  const base = {
    id: "h1",
    text: "Polished words.",
    duration_secs: 3.2,
    timestamp: "2026-09-11T10:00:00Z",
  };

  it("is false without raw_text (no transform ran, or pre-fgm.3 data)", () => {
    expect(isTransformedHistoryEntry(base)).toBe(false);
    expect(isTransformedHistoryEntry({ ...base, raw_text: undefined })).toBe(false);
    expect(isTransformedHistoryEntry({ ...base, raw_text: null })).toBe(false);
  });

  it("is false when raw_text equals the canonical text", () => {
    expect(isTransformedHistoryEntry({ ...base, raw_text: base.text })).toBe(false);
  });

  it("is false for empty or non-string raw_text (defensive against odd data)", () => {
    expect(isTransformedHistoryEntry({ ...base, raw_text: "" })).toBe(false);
    expect(isTransformedHistoryEntry({ ...base, raw_text: 42 })).toBe(false);
    expect(isTransformedHistoryEntry({ ...base, raw_text: { text: "…" } })).toBe(false);
  });

  it("is true when a differing non-empty raw transcript is stored", () => {
    expect(isTransformedHistoryEntry({ ...base, raw_text: "polished words" })).toBe(true);
  });
});

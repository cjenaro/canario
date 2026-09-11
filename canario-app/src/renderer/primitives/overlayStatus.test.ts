// Tests for the overlay status transitions (fgm.4): the event-driven
// lifecycle (hidden ↔ recording → terminal events), the main-process
// status pushes that fill the silent stop→result window, and the busy
// labels — including the new "transforming" phase for the LLM pass.
import { describe, it, expect } from "vitest";
import {
  nextOverlayStatusOnPush,
  overlayBusyLabel,
  overlayStatusFromEvent,
  type OverlayStatus,
} from "./overlayStatus";

describe("overlayStatusFromEvent", () => {
  it.each([
    ["RecordingStarted", "recording"],
    ["RecordingStopped", "hidden"],
    ["TranscriptionReady", "hidden"],
    ["RecordingCancelled", "hidden"],
    ["Error", "hidden"],
  ])("%s → %s", (event, expected) => {
    expect(overlayStatusFromEvent(event)).toBe(expected);
  });

  it.each([
    "AudioLevel",
    "PartialTranscript",
    "ModelDownloadProgress",
    "ModelDownloadComplete",
    "ModelDownloadFailed",
    "HotkeyTriggered",
    // The overlay WINDOW hides via hideOverlay on a crash; the page
    // status is deliberately untouched by this event.
    "SidecarCrashed",
    "SomethingNew",
  ])("leaves the status alone on %s", (event) => {
    expect(overlayStatusFromEvent(event)).toBeNull();
  });
});

describe("nextOverlayStatusOnPush", () => {
  // The pre-fgm.4 seam, unchanged: a successful stop command flips
  // recording → transcribing, and nothing else may.
  it("enters transcribing from recording", () => {
    expect(nextOverlayStatusOnPush("recording", "transcribing")).toBe("transcribing");
  });

  it("ignores a transcribing push from any other state (stale pushes never resurrect the overlay)", () => {
    expect(nextOverlayStatusOnPush("hidden", "transcribing")).toBeNull();
    expect(nextOverlayStatusOnPush("transcribing", "transcribing")).toBeNull();
    expect(nextOverlayStatusOnPush("transforming", "transcribing")).toBeNull();
  });

  // fgm.4: with the transform block enabled, main pushes "transforming"
  // directly on the stop response — accepted straight from recording
  // (the whole stop→result window is labelled) and defensively from
  // transcribing (any future sequencing of the two pushes).
  it("enters transforming from recording (main pushes it directly on stop)", () => {
    expect(nextOverlayStatusOnPush("recording", "transforming")).toBe("transforming");
  });

  it("also advances transcribing → transforming (defensive sequencing)", () => {
    expect(nextOverlayStatusOnPush("transcribing", "transforming")).toBe("transforming");
  });

  it("ignores a transforming push from hidden or already-transforming", () => {
    expect(nextOverlayStatusOnPush("hidden", "transforming")).toBeNull();
    expect(nextOverlayStatusOnPush("transforming", "transforming")).toBeNull();
  });

  it("ignores unknown pushes from every state", () => {
    const states: OverlayStatus[] = ["hidden", "recording", "transcribing", "transforming"];
    for (const s of states) {
      expect(nextOverlayStatusOnPush(s, "downloading")).toBeNull();
      expect(nextOverlayStatusOnPush(s, "")).toBeNull();
    }
  });
});

describe("overlayBusyLabel", () => {
  it("labels both busy phases (same spinner, distinguishing text)", () => {
    expect(overlayBusyLabel("transcribing")).toBe("Transcribing…");
    expect(overlayBusyLabel("transforming")).toBe("Transforming…");
  });

  it("has no label for hidden/recording (the row renders other content)", () => {
    expect(overlayBusyLabel("hidden")).toBeNull();
    expect(overlayBusyLabel("recording")).toBeNull();
  });
});

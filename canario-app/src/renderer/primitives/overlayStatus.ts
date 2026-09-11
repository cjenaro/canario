// Overlay status transitions (canario-fgm.4) — the dictation overlay's
// lifecycle as a pure table, extracted from OverlayPage so it is
// node-testable (mirrors primitives/animations.ts).
//
// Lifecycle: hidden → recording → (stop push) → transcribing |
// transforming → hidden. The sidecar transcribes inside its recording
// thread and emits NOTHING between a successful stop command and the
// terminal TranscriptionReady / RecordingStopped events, so the two
// busy phases are entered via the main process's "overlay:status"
// pushes, not events (main/index.ts onCommandResponse):
//   "transcribing" — the stop→result window with the transform block
//                    off/absent (fgm.1 D5a: today's behavior exactly)
//   "transforming" — the same window with the block enabled: the LLM
//                    pass runs inside it (fgm.1 D1/D3 — the event only
//                    fires after the transform settles or its timeout
//                    falls back to raw, D5d). The transcribe/transform
//                    phase boundary lives in the core and is invisible
//                    to the renderer, so when the pass is on the WHOLE
//                    window is labelled "Transforming…".

/** The overlay island's lifecycle state. */
export type OverlayStatus = "hidden" | "recording" | "transcribing" | "transforming";

/**
 * The status a sidecar event implies for the overlay, or null when the
 * event doesn't move it (AudioLevel, PartialTranscript, model events…).
 */
export function overlayStatusFromEvent(eventName: string): OverlayStatus | null {
  switch (eventName) {
    case "RecordingStarted":
      return "recording";
    case "RecordingStopped":
    case "TranscriptionReady":
    case "RecordingCancelled":
    case "Error":
      // Every pipeline ends in one of these — they dismiss the busy
      // phases exactly as they dismissed "transcribing" before fgm.4.
      return "hidden";
    default:
      return null;
  }
}

/**
 * Apply a main-process "overlay:status" push. Returns the next status,
 * or null when the push must be ignored: pushes only ever follow a
 * successful stop command, so one landing on "hidden" is stale (the
 * result already arrived — never resurrect the overlay) and one naming
 * the current state is a no-op. "transforming" is accepted from both
 * "recording" (main pushes it directly when the transform block is
 * enabled) and "transcribing" (defensive: any future sequencing).
 */
export function nextOverlayStatusOnPush(
  current: OverlayStatus,
  push: string,
): OverlayStatus | null {
  if (push === "transcribing") {
    return current === "recording" ? "transcribing" : null;
  }
  if (push === "transforming") {
    return current === "recording" || current === "transcribing" ? "transforming" : null;
  }
  return null;
}

/**
 * Island label for a busy phase, or null when there is none to show.
 *
 * `labels` defaults to English so this module stays pure and
 * node-testable; OverlayPage passes the i18n catalog's overlay labels
 * so a future locale localizes the pill without this file knowing
 * about i18n.
 */
export interface OverlayBusyLabels {
  transcribing: string;
  transforming: string;
}

export const DEFAULT_OVERLAY_BUSY_LABELS: OverlayBusyLabels = {
  transcribing: "Transcribing…",
  transforming: "Transforming…",
};

export function overlayBusyLabel(
  status: OverlayStatus,
  labels: OverlayBusyLabels = DEFAULT_OVERLAY_BUSY_LABELS,
): string | null {
  switch (status) {
    case "transcribing":
      return labels.transcribing;
    case "transforming":
      return labels.transforming;
    default:
      return null;
  }
}

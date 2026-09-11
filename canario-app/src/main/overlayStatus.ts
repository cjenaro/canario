// Overlay labelling for the stop→result window (canario-fgm.4).
//
// The sidecar transcribes inside its recording thread and emits
// nothing between a successful stop command and the terminal
// TranscriptionReady / RecordingStopped events — main/index.ts fills
// that silence with an "overlay:status" push from onCommandResponse.
// With the transform block enabled (fgm.1 D1/D3) that window also
// contains the sidecar's LLM call, and the transcribe/transform phase
// boundary is invisible from here (the pipeline timing lives in the
// core), so the whole window is labelled "Transforming…" whenever the
// pass is on. Pure so it is node-testable (mirrors hotkeyRouting.ts).

/** The pushes main sends on a successful stop command. */
export type OverlayStatusPush = "transcribing" | "transforming";

/**
 * Read transform.enabled from a config payload (the main-process cache
 * or get_config data). Strict like the renderer's transformFromConfig:
 * an absent, malformed, or non-true block reads as OFF — the safe
 * direction of fgm.1 D5a (default-off).
 */
export function transformEnabledInConfig(config: unknown): boolean {
  const block = ((config ?? {}) as Record<string, unknown>).transform;
  return (
    typeof block === "object" &&
    block !== null &&
    !Array.isArray(block) &&
    (block as Record<string, unknown>).enabled === true
  );
}

/**
 * Which overlay status a successful stop command should push:
 * "transforming" when the transform pass is enabled, "transcribing"
 * otherwise (byte-identical to the pre-fgm.4 behavior).
 */
export function overlayStatusForStop(config: unknown): OverlayStatusPush {
  return transformEnabledInConfig(config) ? "transforming" : "transcribing";
}

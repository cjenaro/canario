// Overlay page — loaded in the overlay BrowserWindow
// Self-contained: listens to sidecar events directly, no state machine needed.
import { createSignal, onCleanup, onMount, Show, For, createEffect } from "solid-js";
// Imperative Motion One core, NOT the <Motion> component wrapper: the
// wrapper bakes a stale snapshot of the initial animate targets into the
// element's reactive style (combineStyle(props.style, createStyles(
// getTarget()))) and re-applies it on every update, stomping running
// geometry springs. Driving animate() directly from the measurement
// effect avoids that entirely.
import { animate, spring } from "@motionone/dom";

type OverlayStatus = "hidden" | "recording" | "transcribing";

export function OverlayPage() {
  const [status, setStatus] = createSignal<OverlayStatus>("hidden");
  const [audioLevel, setAudioLevel] = createSignal(0);
  const [elapsed, setElapsed] = createSignal("0:00");
  const [startedAt, setStartedAt] = createSignal(0);
  const [tick, setTick] = createSignal(0);
  // Live caption preview for long recordings — the latest PartialTranscript
  const [captions, setCaptions] = createSignal<string | null>(null);

  // ── Island geometry ───────────────────────────────────────────────
  // The island's width/height/border-radius are spring-driven via
  // imperative Motion One animate() calls. Targets are measured from the
  // real DOM: the pill hugs the indicator row; the caption card fits the
  // text, capped at 3 lines. The island stays hidden until its first
  // measurement so it never paints at a stale or guessed size.
  const [islandW, setIslandW] = createSignal(120);
  const [islandH, setIslandH] = createSignal(28);
  const [measured, setMeasured] = createSignal(false);
  let islandRef: HTMLDivElement | undefined;
  let rowRef: HTMLDivElement | undefined;
  let captionRef: HTMLDivElement | undefined;
  let islandControls: ReturnType<typeof animate> | undefined;

  // Measure after render. Re-runs as captions grow (retargeting the
  // spring) and as the timer text widens the collapsed pill.
  createEffect(() => {
    const text = captions();
    void elapsed(); // dep: re-measure the pill when the timer ticks over
    void startedAt(); // dep: re-measure every time a recording starts
    void status(); // dep: the island (re)mounts on status changes

    // Measure in a microtask: this effect is created BEFORE the caption
    // <For> below, so it would otherwise run before the words reach the
    // DOM (and before <Show> mounts the island) and read empty boxes.
    queueMicrotask(() => {
      // Skip detached nodes from a previous mount — nothing to size.
      if (!rowRef || !rowRef.isConnected || !islandRef) {
        setMeasured(false);
        return;
      }

      let w: number, h: number;
      if (!text) {
        // Collapsed pill: row content + horizontal chrome (padding 12*2
        // + border 1*2) + vertical chrome (padding 6*2 + border 1*2)
        w = rowRef.offsetWidth + 26;
        h = rowRef.offsetHeight + 14;
      } else {
        // Caption card: fixed 560 wide (536 text + chrome), height fits
        // the text capped at 60px (3 lines) below the indicator row.
        const textH = captionRef ? Math.min(captionRef.offsetHeight, 60) : 0;
        w = 560;
        h = rowRef.offsetHeight + 14 + 6 + textH;
      }

      // Ignore sub-pixel drift so identical re-measures don't re-trigger
      // the animation.
      const wChanged = Math.abs(islandW() - w) >= 1;
      const hChanged = Math.abs(islandH() - h) >= 1;
      if (!wChanged && !hChanged) return;
      setIslandW(w);
      setIslandH(h);

      const radius = text ? "20px" : "9999px";
      if (!measured()) {
        // First sizing after (re)mount: apply instantly while hidden —
        // never spring from a guess or a stale value.
        islandRef.style.width = `${w}px`;
        islandRef.style.height = `${h}px`;
        islandRef.style.borderRadius = radius;
        setMeasured(true);
        return;
      }

      // Stop any in-flight spring, then spring to the new targets.
      islandControls?.stop();
      islandControls = animate(
        islandRef,
        { width: `${w}px`, height: `${h}px`, borderRadius: radius },
        { easing: spring({ stiffness: 180, damping: 22 }) }
      );
    });
  });

  // Force transparent background on the overlay window
  onMount(() => {
    document.documentElement.style.backgroundColor = "transparent";
    document.body.style.backgroundColor = "transparent";
  });

  const api = (window as any).canario as {
    onEvent: (cb: (e: Record<string, unknown>) => void) => () => void;
  } | undefined;

  // ── Listen to sidecar events directly ─────────────────────────────
  onMount(() => {
    if (!api) return;

    const unsub = api.onEvent((event) => {
      const name = event.event as string;

      switch (name) {
        case "RecordingStarted":
          setStatus("recording");
          setStartedAt(Date.now());
          setElapsed("0:00");
          setCaptions(null);
          // Hide until re-measured — never paint a stale island size
          // (e.g. the caption-card width from the previous recording).
          setMeasured(false);
          break;
        // NOTE: the sidecar transcribes in its recording thread and only
        // emits TranscriptionReady (then RecordingStopped) once it's done.
        // RecordingStopped is therefore the FINAL event of every pipeline —
        // the "transcribing" state is entered via the "overlay:status" push
        // from the main process when a stop command succeeds (see below).
        case "RecordingStopped":
        case "TranscriptionReady":
        case "RecordingCancelled":
        case "Error":
          setStatus("hidden");
          setCaptions(null);
          break;
        case "AudioLevel":
          setAudioLevel(event.level as number);
          break;
        // Live preview of a long recording — replaces the whole caption
        // text each time (the core dedupes identical updates). Preview
        // only: the authoritative text arrives via TranscriptionReady.
        case "PartialTranscript":
          setCaptions(event.text as string);
          break;
      }
    });

    onCleanup(unsub);
  });

  // ── Transcribing state pushed by the main process ────────────────
  // Sent when a stop/toggle-stop command succeeds; covers every stop path
  // (tray, global shortcut, UI button, Linux hotkey via HotkeyTriggered).
  onMount(() => {
    const onStatus = (window as any).canario?.onOverlayStatus as
      | ((cb: (status: string) => void) => () => void)
      | undefined;
    if (!onStatus) return;

    const unsub = onStatus((s) => {
      if (s === "transcribing" && status() === "recording") {
        setStatus("transcribing");
      }
    });

    onCleanup(unsub);
  });

  // ── Animation frame loop for waveform ─────────────────────────────
  let rafId: number | null = null;
  onMount(() => {
    function loop() {
      setTick(Date.now());
      rafId = requestAnimationFrame(loop);
    }
    rafId = requestAnimationFrame(loop);
  });
  onCleanup(() => {
    if (rafId != null) cancelAnimationFrame(rafId);
  });

  // ── Elapsed timer ─────────────────────────────────────────────────
  const timer = setInterval(() => {
    if (status() === "recording" && startedAt()) {
      const secs = Math.floor((Date.now() - startedAt()) / 1000);
      const mins = Math.floor(secs / 60);
      const rem = secs % 60;
      setElapsed(`${mins}:${String(rem).padStart(2, "0")}`);
    }
  }, 250);
  onCleanup(() => clearInterval(timer));

  // ── Smoothed audio level ──────────────────────────────────────────
  let smoothLevel = 0;
  const smoothAudio = () => {
    const raw = audioLevel();
    smoothLevel = smoothLevel * 0.6 + raw * 0.4;
    return smoothLevel;
  };

  const hasAudio = () => smoothAudio() > 0.02;

  // ── Waveform bar heights ──────────────────────────────────────────
  const bars = () => {
    tick();
    const level = smoothAudio();
    const now = Date.now();
    const baseHeights = [3, 5, 8, 5, 3];
    return baseHeights.map((base, i) => {
      const wave = Math.sin(now / 150 + i * 1.4) * 0.35 + 0.65;
      const audioBoost = level * 14;
      return Math.max(2, Math.min(16, base * wave + audioBoost));
    });
  };

  const isRecording = () => status() === "recording";
  const isTranscribing = () => status() === "transcribing";
  const isVisible = () => status() !== "hidden";

  // Caption preview as word tokens — <For> keys by word, so words that
  // persist across partial updates keep their DOM and don't re-animate;
  // only newly spoken words fade in.
  const captionWords = () => (captions() ?? "").split(/\s+/).filter(Boolean);

  return (
    <Show when={isVisible()}>
      <div class="fixed inset-0 flex items-start justify-center pt-3 pointer-events-none">
        {/* One island, spring-morphed by Motion One's imperative
            animate() (see the measurement effect): collapsed it's the
            recording pill; live captions spring it open into a wider,
            taller card. Geometry is written only by the animate calls —
            keep it out of this element's reactive style. */}
        <div
          ref={islandRef}
          class="island no-select shadow-2xl"
          classList={{
            "animate-slide-down": measured(),
            "island-live": hasAudio(),
          }}
          style={{
            // Hidden until the first real measurement lands. This is the
            // ONLY reactive key — it flips while the island is invisible.
            visibility: measured() ? "visible" : "hidden",
          }}
        >
          <div class="island-row" ref={rowRef}>
            <Show when={isRecording()}>
              {/* Recording dot */}
              <div
                class="w-2 h-2 rounded-full animate-pulse-dot flex-shrink-0"
                style={{ "background-color": "var(--recording-dot)" }}
              />

              {/* Waveform bars */}
              <div class="flex items-center gap-[2px] h-4">
                <For each={bars()}>
                  {(height) => (
                    <div
                      class="rounded-full"
                      style={{
                        width: "3px",
                        height: `${height}px`,
                        "background-color": hasAudio()
                          ? "var(--accent)"
                          : "rgba(233, 69, 96, 0.35)",
                        transition: "height 80ms ease-out, background-color 200ms ease",
                      }}
                    />
                  )}
                </For>
              </div>

              <span
                class="text-[11px] font-medium tabular-nums flex-shrink-0"
                style={{ color: "rgba(232, 232, 240, 0.9)" }}
              >
                {elapsed()}
              </span>
            </Show>

            <Show when={isTranscribing()}>
              {/* Spinner — same accent treatment as the recording dot */}
              <div
                class="w-3 h-3 rounded-full border-2 border-t-transparent animate-spin flex-shrink-0"
                style={{ "border-color": "rgba(233, 69, 96, 0.35)", "border-top-color": "var(--accent)" }}
              />
              <span
                class="text-[11px] font-medium flex-shrink-0"
                style={{ color: "rgba(232, 232, 240, 0.9)" }}
              >
                Transcribing…
              </span>
            </Show>
          </div>

          {/* Live caption preview — always mounted so it can be measured
              and clipped by the island's spring-animated box. Springs
              open with the island and stays up through the "transcribing"
              phase; clears with the final result. */}
          <div class="caption-text" ref={captionRef}>
            <div>
              <For each={captionWords()}>
                {(word) => <span class="caption-word">{word}</span>}
              </For>
            </div>
          </div>
        </div>
      </div>
    </Show>
  );
}

// Overlay page — loaded in the overlay BrowserWindow
// Self-contained: listens to sidecar events directly, no state machine needed.
import { createSignal, onCleanup, onMount, Show, For } from "solid-js";

type OverlayStatus = "hidden" | "recording" | "transcribing";

export function OverlayPage() {
  const [status, setStatus] = createSignal<OverlayStatus>("hidden");
  const [audioLevel, setAudioLevel] = createSignal(0);
  const [elapsed, setElapsed] = createSignal("0:00");
  const [startedAt, setStartedAt] = createSignal(0);
  const [tick, setTick] = createSignal(0);

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
          break;
        case "RecordingStopped":
        case "TranscriptionReady":
        case "Error":
          setStatus("hidden");
          break;
        case "AudioLevel":
          setAudioLevel(event.level as number);
          break;
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
  const isVisible = () => status() !== "hidden";

  return (
    <Show when={isVisible()}>
      <div class="fixed inset-0 flex items-start justify-center pt-3 pointer-events-none">
        <div
          class="flex items-center gap-2 px-3 py-1.5 rounded-full shadow-2xl no-select animate-slide-down"
          style={{
            "background-color": "rgba(26, 26, 46, 0.92)",
            "backdrop-filter": "blur(12px)",
            border: "1px solid rgba(233, 69, 96, 0.3)",
            "box-shadow": hasAudio()
              ? "0 0 12px rgba(233, 69, 96, 0.25)"
              : "0 4px 20px rgba(0, 0, 0, 0.3)",
            transition: "box-shadow 200ms ease",
          }}
        >
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
          </Show>

          <span
            class="text-[11px] font-medium tabular-nums flex-shrink-0"
            style={{ color: "rgba(232, 232, 240, 0.9)" }}
          >
            {elapsed()}
          </span>
        </div>
      </div>
    </Show>
  );
}

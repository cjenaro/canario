// Recording overlay — the floating indicator that shows during recording
import { createSignal, onCleanup, Show, createEffect } from "solid-js";
import { useAppState } from "../state/context";

export function RecordingOverlay() {
  const { state } = useAppState();
  const [audioLevel, setAudioLevel] = createSignal(0);
  const [elapsed, setElapsed] = createSignal("0:00");
  const [visible, setVisible] = createSignal(false);
  const [hiding, setHiding] = createSignal(false);

  // Sync visibility with recording/transcribing state
  createEffect(() => {
    const s = state();
    if (s.status === "recording" || s.status === "transcribing") {
      setHiding(false);
      setVisible(true);
    } else {
      // Animate out, then hide
      setHiding(true);
      const t = setTimeout(() => {
        setVisible(false);
        setHiding(false);
      }, 100);
      onCleanup(() => clearTimeout(t));
    }
  });

  // Listen for audio level events
  function onAudioLevel(e: Event) {
    const level = (e as CustomEvent).detail as number;
    setAudioLevel(level);
  }
  window.addEventListener("canario:audiolevel", onAudioLevel);
  onCleanup(() => window.removeEventListener("canario:audiolevel", onAudioLevel));

  // Elapsed timer
  const interval = setInterval(() => {
    const s = state();
    if (s.status === "recording") {
      const secs = Math.floor((Date.now() - s.startedAt) / 1000);
      const mins = Math.floor(secs / 60);
      const remaining = secs % 60;
      setElapsed(`${mins}:${String(remaining).padStart(2, "0")}`);
    }
  }, 200);
  onCleanup(() => clearInterval(interval));

  const isRecording = () => state().status === "recording";
  const isTranscribing = () => state().status === "transcribing";

  return (
    <Show when={visible()}>
      <div
        class="fixed top-2 left-1/2 -translate-x-1/2 z-50"
        classList={{
          "animate-slide-down": !hiding(),
          "animate-fade-out": hiding(),
        }}
      >
        <div
          class="flex items-center gap-3 px-4 py-2.5 rounded-xl shadow-2xl no-select"
          style={{
            "background-color": "var(--surface)",
            "backdrop-filter": "blur(12px)",
            border: "1px solid var(--border)",
            "min-width": "220px",
          }}
        >
          <Show when={isRecording()} fallback={
            // Transcribing indicator
            <div class="w-3 h-3 rounded-full border-2 border-t-transparent animate-spin"
              style={{ "border-color": "var(--accent)", "border-top-color": "transparent" }} />
          }>
            {/* Recording dot */}
            <div class="w-3 h-3 rounded-full animate-pulse-dot"
              style={{ "background-color": "var(--recording-dot)" }} />
          </Show>

          <div class="flex-1 flex flex-col gap-1">
            <div class="flex items-center justify-between">
              <span class="text-xs font-medium" style={{ color: "var(--text-primary)" }}>
                <Show when={isRecording()} fallback="Transcribing…">Recording</Show>
              </span>
              <Show when={isRecording()}>
                <span class="text-xs tabular-nums" style={{ color: "var(--text-secondary)" }}>
                  {elapsed()}
                </span>
              </Show>
            </div>
            <Show when={isRecording()}>
              {/* Audio level bar */}
              <div class="h-1.5 rounded-full overflow-hidden" style={{ "background-color": "var(--border)" }}>
                <div
                  class="h-full rounded-full"
                  style={{
                    width: `${Math.max(2, audioLevel() * 100)}%`,
                    "background-color": audioLevel() > 0.8 ? "var(--error)" : "var(--accent)",
                    transition: "width 50ms linear",
                  }}
                />
              </div>
            </Show>
          </div>
        </div>
      </div>
    </Show>
  );
}

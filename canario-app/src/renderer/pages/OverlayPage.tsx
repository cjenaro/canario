// Overlay page — loaded in the overlay BrowserWindow
// Self-contained: listens to sidecar events directly, no state machine needed.
import { createSignal, onCleanup, onMount, Show, For, createEffect, createMemo } from "solid-js";
// Imperative Motion One core, NOT the <Motion> component wrapper: the
// wrapper bakes a stale snapshot of the initial animate targets into the
// element's reactive style (combineStyle(props.style, createStyles(
// getTarget()))) and re-applies it on every update, stomping running
// geometry springs. Driving animate() directly from the measurement
// effect avoids that entirely.
import { animate, spring } from "@motionone/dom";
// Per-monitor drag placement: pure offset math + AppConfig serialization.
// See canario-aud.1 and the drag-affordance notes further below.
import {
  applyDragDelta,
  clampOverlayPosition,
  overlayOffsetsConfigPayload,
  overlayOffsetsFromConfig,
  resolveOverlayPosition,
  toStorableOffset,
  withOverlayOffset,
  withoutOverlayOffset,
  type OverlayOffset,
  type OverlayOffsets,
} from "../primitives/overlayPlacement";
// Overlay lifecycle states + the pure transition table (fgm.4) —
// "transforming" mirrors "transcribing" for the stop→result window
// that contains the LLM pass when the transform block is enabled.
import {
  nextOverlayStatusOnPush,
  overlayBusyLabel,
  overlayStatusFromEvent,
  type OverlayBusyLabels,
  type OverlayStatus,
} from "../primitives/overlayStatus";
// Indicator presence modes + the pure mode×status render-gating table
// (canario-aud.2): "full" paints the island below, "dot" a minimal
// pulsing dot while recording, "tray" nothing (the main process keeps
// the window hidden in that mode — this is the renderer's half).
import {
  DOT_SIZE,
  normalizeOverlayPresence,
  overlayContentFor,
  overlayPresenceFromConfig,
  type OverlayPresence,
} from "../primitives/overlayPresence";
import { t } from "../i18n";

/** Busy-phase labels from the i18n catalog — the locale-aware half of
 *  primitives/overlayStatus.ts's overlayBusyLabel (see that file). */
const busyLabels = (): OverlayBusyLabels => ({
  transcribing: t("overlay.transcribing"),
  transforming: t("overlay.transforming"),
});

/** Island rect in window-relative client coords, reported to the main process. */
type IslandRect = { x: number; y: number; width: number; height: number };

// Pointer travel (px) before a press counts as a drag — a stray click on
// the island must not write a config update.
const DRAG_THRESHOLD_PX = 3;

export function OverlayPage() {
  const [status, setStatus] = createSignal<OverlayStatus>("hidden");
  const [audioLevel, setAudioLevel] = createSignal(0);
  const [elapsed, setElapsed] = createSignal("0:00");
  const [startedAt, setStartedAt] = createSignal(0);
  const [tick, setTick] = createSignal(0);
  // Live caption preview for long recordings — the latest PartialTranscript
  const [captions, setCaptions] = createSignal<string | null>(null);

  // ── Indicator presence (canario-aud.2) ────────────────────────────
  // Which indicator to paint: pulled from get_config at mount, then
  // kept current by the main process's overlay:mode pushes (settings
  // changes apply live). See overlayContentFor for the gating table.
  const [presence, setPresence] = createSignal<OverlayPresence>("full");

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
    sendCommand: (cmd: Record<string, unknown>) => Promise<Record<string, unknown> | null>;
    updateConfigCache: (config: Record<string, unknown>) => Promise<void>;
    setOverlayIslandRect: (rect: IslandRect | null) => void;
    onOverlayDisplay: (cb: (info: { key: string }) => void) => () => void;
    onOverlayInteractive: (cb: (interactive: boolean) => void) => () => void;
    onOverlayMode: (cb: (mode: string) => void) => () => void;
  } | undefined;

  // ── Island placement (canario-aud.1) ──────────────────────────────
  // Drag-to-move + per-monitor persistence. The overlay window stays
  // click-through; the MAIN process polls the cursor against the island
  // rect we push below and enables mouse events only while the cursor is
  // over the island (forwarded mousemove doesn't exist on Linux —
  // electron#16777 — so hover detection can't live here). While it deems
  // us interactive we get real pointer events and the island becomes a
  // drag handle; everything else keeps passing clicks through.
  // The identity of the monitor the overlay currently covers — pushed by
  // the main process on every show (frontend-agnostic key from
  // monitorIdentity.ts: xrandr-style label or bounds composite). Keys our
  // slice of the persisted overlay_offsets map.
  const [monitorKey, setMonitorKey] = createSignal<string | null>(null);
  const [interactive, setInteractive] = createSignal(false);
  const [storedOffsets, setStoredOffsets] = createSignal<OverlayOffsets>({});
  const [dragging, setDragging] = createSignal(false);
  const [dragPos, setDragPos] = createSignal<OverlayOffset | null>(null);
  // Viewport size (= the overlay window, which exactly covers one
  // display) for default centering and clamp math. Re-read on resize —
  // positionOverlayWindow() re-bounds the window onto the display under
  // the cursor, which can differ in size.
  const [vw, setVw] = createSignal(window.innerWidth);
  const [vh, setVh] = createSignal(window.innerHeight);

  let dragStartPointer: { x: number; y: number } | null = null;
  let dragStartPos: OverlayOffset | null = null;
  let dragMoved = false;

  // Resolved island top-left (client coords = display-relative DIPs).
  // During a drag it's the live clamped pointer delta; otherwise the
  // stored offset for THIS monitor, falling back to the default
  // top-center, re-clamped so a stale offset can't land off-screen
  // (the monitor may have changed since the drag).
  const pos = createMemo<OverlayOffset>(() => {
    const live = dragPos();
    if (dragging() && live) return live;
    const stored = storedOffsets()[monitorKey() ?? ""];
    const base = resolveOverlayPosition(stored, vw(), islandW());
    return clampOverlayPosition(base, { width: vw(), height: vh() }, { width: islandW(), height: islandH() });
  });

  // Minimal dot placement (canario-aud.2): the SAME per-monitor stored
  // offsets and default top-center as the island — the map is shared,
  // so a monitor dragged in full mode anchors the dot near that spot —
  // resolved/clamped for the dot's tiny fixed size. The dot itself is
  // NOT draggable: the hover-hysteresis drag affordance is sized for
  // the island (24px exit margin vs a 10px target), so dot mode fixes
  // the position; reposition by switching to (or via) the full overlay,
  // whose drags persist into the shared map.
  const dotPos = createMemo<OverlayOffset>(() => {
    const stored = storedOffsets()[monitorKey() ?? ""];
    const base = resolveOverlayPosition(stored, vw(), DOT_SIZE);
    return clampOverlayPosition(base, { width: vw(), height: vh() }, { width: DOT_SIZE, height: DOT_SIZE });
  });

  // What to paint for the current mode × status (canario-aud.2). "full"
  // keeps today's island behavior; "dot" paints the recording-only dot;
  // "tray" paints nothing (the main process also keeps the window
  // hidden there — belt and braces on both sides of the bridge).
  const content = createMemo(() => overlayContentFor(presence(), status()));

  // Report the island's rect whenever it moves or resizes — the main
  // process hit-tests the cursor against it to toggle interactivity.
  // null while the island is hidden stops that polling entirely (the
  // dot mode has no drag affordance, so it reports nothing).
  createEffect(() => {
    const rect: IslandRect | null =
      content().island && isVisible() && measured()
        ? { x: pos().x, y: pos().y, width: islandW(), height: islandH() }
        : null;
    api?.setOverlayIslandRect?.(rect);
  });

  // Persist the full per-monitor map (update_config merges top-level
  // keys wholesale, so partial maps would clobber other monitors).
  async function sendPlacementUpdate(next: OverlayOffsets) {
    setStoredOffsets(next);
    const payload = overlayOffsetsConfigPayload(next);
    try {
      await api?.sendCommand({ id: `overlay-place-${Date.now()}`, cmd: "update_config", config: payload });
      await api?.updateConfigCache?.(payload);
    } catch (err) {
      console.error("[overlay] placement save failed:", err);
    }
  }

  function persistPlacement(finalPos: OverlayOffset) {
    const key = monitorKey();
    if (!key) return; // no display push yet — visual only, reverts next show
    void sendPlacementUpdate(withOverlayOffset(storedOffsets(), key, toStorableOffset(finalPos)));
  }

  // Reset-to-default: drop THIS monitor's entry (others survive) and
  // snap back to the top-center default via the pos() memo.
  function resetPlacement() {
    const key = monitorKey();
    if (!key || !(key in storedOffsets())) return;
    void sendPlacementUpdate(withoutOverlayOffset(storedOffsets(), key));
  }

  function endDrag(persist = true) {
    const wasDragging = dragging();
    const finalPos = dragPos();
    dragStartPointer = null;
    dragStartPos = null;
    // Below-threshold presses are clicks, not drags — never persist those.
    const moved = dragMoved && finalPos !== null;
    dragMoved = false;
    setDragging(false);
    setDragPos(null);
    if (wasDragging && persist && moved) persistPlacement(finalPos);
  }

  // ── Placement listeners ───────────────────────────────────────────
  onMount(() => {
    // Load persisted placements and the indicator mode once; drag/reset
    // keep the map current locally so consecutive updates never read
    // back stale config.
    api
      ?.sendCommand({ id: "overlay-config", cmd: "get_config" })
      .then((res) => {
        if (res?.ok && res.data) {
          setStoredOffsets(overlayOffsetsFromConfig(res.data));
          // The pull covers mode pushes that raced this page's load
          // (main refreshes its config cache before the window loads).
          setPresence(overlayPresenceFromConfig(res.data));
        }
      })
      .catch(() => {});

    // Which display the overlay landed on (pushed on every show) — the
    // monitor identity key for placement persistence
    const unsubDisplay = api?.onOverlayDisplay?.((info) => setMonitorKey(info.key));
    // Interactive ↔ click-through flips from the main process
    const unsubInteractive = api?.onOverlayInteractive?.((i) => {
      setInteractive(i);
      // Lost mid-drag (e.g. the button was released outside the window):
      // settle where the island is rather than reverting.
      if (!i) endDrag();
    });
    // Indicator mode changes (settings writes / external config edits)
    // pushed live by the main process
    const unsubMode = api?.onOverlayMode?.((m) => setPresence(normalizeOverlayPresence(m)));

    const onResize = () => {
      setVw(window.innerWidth);
      setVh(window.innerHeight);
    };
    window.addEventListener("resize", onResize);

    onCleanup(() => {
      unsubDisplay?.();
      unsubInteractive?.();
      unsubMode?.();
      window.removeEventListener("resize", onResize);
      api?.setOverlayIslandRect?.(null);
    });
  });

  // Recording ended mid-drag: cancel without persisting (the island is
  // unmounting anyway) and drop any stale interactive state.
  createEffect(() => {
    if (!isVisible()) {
      endDrag(false);
      setInteractive(false);
    }
  });

  // ── Listen to sidecar events directly ─────────────────────────────
  onMount(() => {
    if (!api) return;

    const unsub = api.onEvent((event) => {
      const name = event.event as string;

      // Lifecycle events map through the pure transition table
      // (overlayStatus.ts): RecordingStarted flips us to recording;
      // every pipeline's terminal events (TranscriptionReady /
      // RecordingStopped / RecordingCancelled / Error) dismiss the
      // island — including from the "transforming" phase, exactly as
      // they dismissed "transcribing" before fgm.4. If no terminal
      // event ever arrived the island would persist, same as today;
      // the sidecar's transform timeout (fgm.1 D5d) guarantees one.
      const next = overlayStatusFromEvent(name);
      if (next !== null) {
        setStatus(next);
        if (next === "recording") {
          setStartedAt(Date.now());
          setElapsed("0:00");
          // Hide until re-measured — never paint a stale island size
          // (e.g. the caption-card width from the previous recording).
          setMeasured(false);
        }
        setCaptions(null);
        return;
      }

      switch (name) {
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

  // ── Busy phases pushed by the main process ───────────────────────
  // Sent when a stop/toggle-stop command succeeds; covers every stop path
  // (tray, global shortcut, UI button, Linux hotkey via HotkeyTriggered).
  // "transcribing" labels the silent stop→result window as before;
  // "transforming" (fgm.4) is what main pushes when the transform block
  // is enabled — the sidecar's LLM pass runs in that same window, before
  // TranscriptionReady fires. The transition table ignores pushes that
  // would resurrect a hidden overlay (stale) or repeat the current state.
  onMount(() => {
    const onStatus = (window as any).canario?.onOverlayStatus as
      | ((cb: (status: string) => void) => () => void)
      | undefined;
    if (!onStatus) return;

    const unsub = onStatus((s) => {
      const next = nextOverlayStatusOnPush(status(), s);
      if (next !== null) {
        setStatus(next);
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
  // Both post-stop phases share one visual (spinner + label); the
  // label distinguishes them (fgm.4: "Transforming…" for the LLM pass).
  const isBusy = () => status() === "transcribing" || status() === "transforming";
  const isVisible = () => status() !== "hidden";

  // Caption preview as word tokens — <For> keys by word, so words that
  // persist across partial updates keep their DOM and don't re-animate;
  // only newly spoken words fade in.
  const captionWords = () => (captions() ?? "").split(/\s+/).filter(Boolean);

  return (
    <Show when={content().island || content().dot}>
      <div class="fixed inset-0 pointer-events-none">
        {/* ── Full island (mode "full", canario-aud.2) ───────────────
            Unchanged behavior: visible for every non-hidden status. */}
        <Show when={content().island}>
        {/* Placement wrapper: absolute left/top from the pos() memo —
            default top-center, the stored per-monitor offset, or the
            live drag position. Keeping positioning here leaves the
            island's spring-driven width/height untouched. */}
        <div
          style={{
            position: "absolute",
            left: `${pos().x}px`,
            top: `${pos().y}px`,
          }}
        >
          {/* One island, spring-morphed by Motion One's imperative
              animate() (see the measurement effect): collapsed it's the
              recording pill; live captions spring it open into a wider,
              taller card. Geometry is written only by the animate calls —
              keep it out of this element's reactive style.
              Drag affordance: pointer events only ever arrive while the
              main process deems the window interactive (cursor over the
              island) — see the placement section above. */}
          <div
            ref={islandRef}
            class="island no-select shadow-2xl pointer-events-auto"
            classList={{
              "animate-slide-down": measured(),
              "island-live": hasAudio(),
              "cursor-grab": interactive() && !dragging(),
              "cursor-grabbing": dragging(),
            }}
            style={{
              // Hidden until the first real measurement lands. This is the
              // ONLY reactive key — it flips while the island is invisible.
              visibility: measured() ? "visible" : "hidden",
            }}
            title={t("overlay.dragTitle")}
            onPointerDown={(e) => {
              // Left button only, and only while the window is interactive
              if (e.button !== 0 || !interactive()) return;
              dragStartPointer = { x: e.clientX, y: e.clientY };
              dragStartPos = { ...pos() };
              dragMoved = false;
              setDragging(true);
              setDragPos(dragStartPos);
              // Capture so pointermove/up keep flowing to the island even
              // when the cursor outruns it mid-drag.
              try {
                islandRef?.setPointerCapture(e.pointerId);
              } catch { /* capture is best-effort */ }
            }}
            onPointerMove={(e) => {
              if (!dragging() || !dragStartPointer || !dragStartPos) return;
              const dx = e.clientX - dragStartPointer.x;
              const dy = e.clientY - dragStartPointer.y;
              if (!dragMoved && Math.hypot(dx, dy) >= DRAG_THRESHOLD_PX) dragMoved = true;
              setDragPos(
                clampOverlayPosition(
                  applyDragDelta(dragStartPos, dx, dy),
                  { width: vw(), height: vh() },
                  { width: islandW(), height: islandH() },
                ),
              );
            }}
            onPointerUp={(e) => {
              try {
                islandRef?.releasePointerCapture(e.pointerId);
              } catch { /* not captured */ }
              endDrag();
            }}
            onPointerCancel={() => endDrag(false)}
            onDblClick={() => resetPlacement()}
            onContextMenu={(e) => {
              // Right-click doubles as reset (and must not open a menu
              // over the transparent overlay).
              e.preventDefault();
              resetPlacement();
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

            <Show when={isBusy()}>
              {/* Spinner — same accent treatment as the recording dot;
                  the label distinguishes the two busy phases. */}
              <div
                class="w-3 h-3 rounded-full border-2 border-t-transparent animate-spin flex-shrink-0"
                style={{ "border-color": "rgba(233, 69, 96, 0.35)", "border-top-color": "var(--accent)" }}
              />
              <span
                class="text-[11px] font-medium flex-shrink-0"
                style={{ color: "rgba(232, 232, 240, 0.9)" }}
              >
                {overlayBusyLabel(status(), busyLabels())}
              </span>
            </Show>
          </div>

          {/* Live caption preview — always mounted so it can be measured
              and clipped by the island's spring-animated box. Springs
              open with the island and stays up through the busy
              (transcribing / transforming) phases; clears with the
              final result. */}
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

        {/* ── Minimal recording dot (mode "dot", canario-aud.2) ──────
            A tiny pulsing dot at the shared placement anchor —
            recording state ONLY: no captions, no timer, no
            transcribing/transforming phases (overlayContentFor gates
            it). Fixed position (see dotPos above for why it isn't
            draggable); pointer-events stay off — the window is
            click-through everywhere in this mode. The pulse reuses
            animate-pulse-dot, so the Motion section's recording-dot
            toggle and the OS reduced-motion request gate it too. */}
        <Show when={content().dot}>
          <div
            style={{
              position: "absolute",
              left: `${dotPos().x}px`,
              top: `${dotPos().y}px`,
              width: `${DOT_SIZE}px`,
              height: `${DOT_SIZE}px`,
            }}
          >
            <div
              class="w-full h-full rounded-full animate-pulse-dot shadow-lg"
              style={{ "background-color": "var(--recording-dot)" }}
            />
          </div>
        </Show>
      </div>
    </Show>
  );
}

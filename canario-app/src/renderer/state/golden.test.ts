// Golden-trace replay (canario-dmp.13, renderer half) + wire-event
// coverage pin.
//
// The fixtures in ./golden/*.json are a FROZEN shared contract — do not
// edit them. The Rust suite (canario-core) validates that they are real
// wire shapes with core invariants; this suite replays each one through
// the renderer exactly the way createCanario's live subscription does —
// via translateSidecarEvent into a real createAppMachine — and asserts
// the machine's landing state and the overlay effects.
//
// A new fixture file without an EXPECTATIONS entry below fails this
// suite: new behavior gets a new fixture (with Rust-suite coverage) AND
// a renderer-side expectation here.
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { createAppMachine, type AppMachine } from "./machine";
import {
  getUnhandledSidecarEventNames,
  resetUnhandledSidecarEventNames,
  translateSidecarEvent,
  type SidecarEventDeps,
} from "../primitives/createCanario";

interface GoldenTrace {
  name: string;
  description: string;
  events: Record<string, unknown>[];
}

const goldenDir = fileURLToPath(new URL("./golden", import.meta.url));

function loadTraces(): GoldenTrace[] {
  return readdirSync(goldenDir)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .map((f) => JSON.parse(readFileSync(join(goldenDir, f), "utf-8")) as GoldenTrace);
}

/**
 * A replay harness: a real machine, recorded overlay effects, no
 * Electron. `modelPresent` is what the sidecar would answer to
 * is_model_downloaded (drives checkModel after download terminals).
 */
function harness(modelPresent = false) {
  const machine = createAppMachine();
  const api = {
    showOverlay: vi.fn(() => Promise.resolve()),
    hideOverlay: vi.fn(() => Promise.resolve()),
  };
  const deps: SidecarEventDeps = {
    send: machine.send,
    updateContext: machine.updateContext,
    api,
    getConfig: vi.fn(async () => undefined),
    // Mirrors the real checkModel: re-derives readiness from core truth
    // into the machine's context.
    checkModel: vi.fn(async () => {
      machine.updateContext({ modelReady: modelPresent });
      return modelPresent;
    }),
    notifyDownloadComplete: vi.fn(),
    notifyTransformFallback: vi.fn(),
    toggleRecording: vi.fn(),
  };
  return { machine, api, deps };
}

async function replay(trace: GoldenTrace, machine: AppMachine, deps: SidecarEventDeps) {
  for (const ev of trace.events) {
    // The stop RESPONSE precedes the RecordingStopped event in the live
    // bridge: stop_recording always acks ok — even for too-short
    // captures (canario-electron handle_command) — and stopRecording()
    // sends STOP_RECORDING on res.ok. An events-only trace can't carry
    // responses, so model that one here; from `transcribing` or `idle`
    // the extra STOP_RECORDING is ignored, exactly as live.
    if (ev.event === "RecordingStopped") {
      machine.send({ type: "STOP_RECORDING" });
    }
    translateSidecarEvent(ev, deps);
  }
  // Let fire-and-forget effects (checkModel after download terminals)
  // settle so context assertions are deterministic.
  await new Promise((resolve) => setTimeout(resolve, 0));
}

// The fixture's authoritative transcript (absent on cancel / too-short /
// error traces).
function readyText(trace: GoldenTrace): string | undefined {
  return trace.events.find((e) => e.event === "TranscriptionReady")?.text as string | undefined;
}

interface ReplayExpectation {
  /** Seed the machine the way the live bridge would be when the trace starts. */
  seed: "model" | "download";
  /** Sidecar's is_model_downloaded answer (download traces). */
  modelPresent?: boolean;
  /** Exact overlay-effect counts pinned from the switch's behavior. */
  overlays: { show: number; hide: number };
  assert: (trace: GoldenTrace, machine: AppMachine) => void;
}

const EXPECTATIONS: Record<string, ReplayExpectation> = {
  "record_transcribe": {
    seed: "model",
    overlays: { show: 1, hide: 2 },
    assert: (t, m) => {
      expect(m.state().status).toBe("idle");
      expect(m.context().lastTranscription).toBe(readyText(t));
      expect(m.context().lastDuration).toBe(2.5);
      expect(m.context().lastError).toBeNull();
    },
  },
  "record_transform": {
    seed: "model",
    overlays: { show: 1, hide: 2 },
    assert: (t, m) => {
      // The machine stores only the canonical text; raw_text is the
      // history/affordance concern (fgm.3 D3).
      expect(m.state().status).toBe("idle");
      expect(m.context().lastTranscription).toBe(readyText(t));
      expect(m.context().lastDuration).toBe(1.8);
      expect(m.context().lastError).toBeNull();
    },
  },
  "record_cancel": {
    seed: "model",
    overlays: { show: 1, hide: 1 },
    assert: (_t, m) => {
      // Audio discarded: no transcript, no error, back to idle.
      expect(m.state().status).toBe("idle");
      expect(m.context().lastTranscription).toBeNull();
      expect(m.context().lastDuration).toBeNull();
      expect(m.context().lastError).toBeNull();
    },
  },
  "record_too_short": {
    seed: "model",
    overlays: { show: 1, hide: 1 },
    assert: (_t, m) => {
      // Discarded by the too-short guard: never transcribed, back to
      // idle (RecordingStopped is the terminal — and only — event).
      expect(m.state().status).toBe("idle");
      expect(m.context().lastTranscription).toBeNull();
      expect(m.context().lastDuration).toBeNull();
    },
  },
  "record_live_captions": {
    seed: "model",
    overlays: { show: 1, hide: 2 },
    assert: (t, m) => {
      // The partial previews leave no trace: the authoritative
      // TranscriptionReady text is what lands in context.
      expect(m.state().status).toBe("idle");
      expect(m.context().lastTranscription).toBe(readyText(t));
      expect(m.context().lastTranscription).not.toContain("live preview");
      expect(m.context().lastDuration).toBe(11.2);
    },
  },
  "record_error": {
    seed: "model",
    overlays: { show: 1, hide: 1 },
    assert: (t, m) => {
      expect(m.state().status).toBe("idle");
      expect(m.context().lastError).toBe(
        t.events.find((e) => e.event === "Error")?.message
      );
      expect(m.context().lastTranscription).toBeNull();
    },
  },
  "download_complete": {
    seed: "download",
    modelPresent: true,
    overlays: { show: 0, hide: 0 },
    assert: (_t, m) => {
      expect(m.state().status).toBe("idle");
      expect(m.context().modelReady).toBe(true);
    },
  },
  "download_cancel": {
    seed: "download",
    modelPresent: false,
    overlays: { show: 0, hide: 0 },
    assert: (_t, m) => {
      // Cancelled download: .part files kept for resume, but no model
      // is usable — readiness stays false.
      expect(m.state().status).toBe("idle");
      expect(m.context().modelReady).toBe(false);
    },
  },
};

describe("golden trace replay (canario-dmp.13)", () => {
  const traces = loadTraces();

  it("every fixture has an expectation entry (and vice versa)", () => {
    expect(traces.map((t) => t.name)).toEqual(Object.keys(EXPECTATIONS).sort());
  });

  for (const trace of traces) {
    it(`replays ${trace.name} — ${trace.description}`, async () => {
      const exp = EXPECTATIONS[trace.name];
      expect(exp, `no EXPECTATIONS entry for fixture "${trace.name}"`).toBeDefined();

      const { machine, api, deps } = harness(exp.modelPresent ?? false);
      if (exp.seed === "model") {
        // Recording is only possible with a model present (the machine's
        // START_RECORDING guard) — the live app checks the model at boot.
        machine.updateContext({ modelReady: true });
      } else {
        // The download was started via the downloadModel command path.
        machine.send({ type: "START_DOWNLOAD" });
      }

      await replay(trace, machine, deps);

      exp.assert(trace, machine);
      expect(api.showOverlay).toHaveBeenCalledTimes(exp.overlays.show);
      expect(api.hideOverlay).toHaveBeenCalledTimes(exp.overlays.hide);
    });
  }

  it("PartialTranscript alone leaves no trace in the machine", () => {
    const { machine, deps } = harness(true);
    translateSidecarEvent({ event: "PartialTranscript", text: "preview only" }, deps);
    expect(machine.state().status).toBe("idle");
    expect(machine.context().lastTranscription).toBeNull();
    expect(machine.context().lastDuration).toBeNull();
    expect(machine.context().lastError).toBeNull();
  });
});

// ── Wire-event coverage pin ─────────────────────────────────────────────
//
// MAINTENANCE RULE: the 13 core names below come from canario-core's
// `Event` enum (src/event.rs — the serde `event` tag); SidecarCrashed is
// synthesized by the Electron main process when the sidecar dies. When a
// new core event is added, the first test below FAILS until its name is
// added to CORE_WIRE_EVENTS here — and the second FAILS until the
// renderer handles it in createCanario's translateSidecarEvent switch or
// it is added to IGNORED_BY_MACHINE with a comment saying why not. A core
// event the renderer silently drops is a lost feature, not a no-op.
describe("renderer wire-event coverage", () => {
  const CORE_WIRE_EVENTS = [
    "RecordingStarted",
    "RecordingStopped",
    "RecordingCancelled",
    "TranscriptionStarted",
    "TranscriptionReady",
    "Error",
    "AudioLevel",
    "PartialTranscript",
    "ModelDownloadProgress",
    "ModelDownloadComplete",
    "ModelDownloadFailed",
    "ConfigChanged",
    "HotkeyTriggered",
  ] as const;

  // Synthesized in Electron main (sidecar.ts), never emitted by core.
  const ELECTRON_SYNTHESIZED_EVENTS = ["SidecarCrashed"] as const;

  // Wire events the renderer deliberately ignores today:
  // • PartialTranscript — live-caption preview; the machine only consumes
  //   the authoritative TranscriptionReady (see event.rs docs). It falls
  //   through to the switch's default arm, so it must be listed here.
  //
  // Not on this list: AudioLevel is likewise a no-op today, but via an
  // explicit `case` in the switch (acknowledged, not ignored);
  // HotkeyTriggered IS handled — the Linux sidecar-hotkey path toggles
  // recording from that very case (Electron-main hotkeys on
  // macOS/Windows arrive through the separate onHotkey channel instead).
  const IGNORED_BY_MACHINE = new Set(["PartialTranscript"]);

  /**
   * Variant → serde-tag names parsed from canario-core's Event enum
   * source, so the pinned CORE_WIRE_EVENTS list can't drift from the
   * Rust truth (handles `#[serde(rename = "...")]` variants).
   */
  function parseCoreEventNames(source: string): string[] {
    const enumStart = source.indexOf("pub enum Event {");
    expect(enumStart, "canario-core/src/event.rs: `pub enum Event {` not found").toBeGreaterThan(-1);
    const bodyStart = source.indexOf("{", enumStart);
    let depth = 0;
    let end = -1;
    for (let i = bodyStart; i < source.length; i++) {
      if (source[i] === "{") depth++;
      else if (source[i] === "}") {
        depth--;
        if (depth === 0) {
          end = i;
          break;
        }
      }
    }
    expect(end, "canario-core/src/event.rs: unterminated Event enum").toBeGreaterThan(-1);
    // Strip comments before splitting — doc comments would read as
    // variant identifiers.
    const body = source.slice(bodyStart + 1, end).replace(/\/\/[^\n]*/g, "");
    // Split top-level variants on commas (payloads nest in braces).
    const chunks: string[] = [];
    let current = "";
    let nesting = 0;
    for (const ch of body) {
      if (ch === "{" || ch === "(") nesting++;
      else if (ch === "}" || ch === ")") nesting--;
      if (ch === "," && nesting === 0) {
        chunks.push(current);
        current = "";
      } else {
        current += ch;
      }
    }
    if (current.trim()) chunks.push(current);
    return chunks.map((chunk) => {
      const rename = chunk.match(/#\[serde\(\s*rename\s*=\s*"([^"]+)"\s*\)\]/);
      if (rename) return rename[1];
      const ident = chunk.match(/([A-Za-z_][A-Za-z0-9_]*)/);
      if (!ident) throw new Error(`could not parse an event variant from: ${chunk.trim()}`);
      return ident[1];
    });
  }

  it("CORE_WIRE_EVENTS matches canario-core's Event enum", () => {
    const eventRs = fileURLToPath(new URL("../../../../canario-core/src/event.rs", import.meta.url));
    const parsed = parseCoreEventNames(readFileSync(eventRs, "utf-8"));
    expect([...parsed].sort()).toEqual([...CORE_WIRE_EVENTS].sort());
  });

  it("every wire event is handled by the switch or explicitly ignored", () => {
    resetUnhandledSidecarEventNames();
    const { deps } = harness(true);
    const everyName = [...CORE_WIRE_EVENTS, ...ELECTRON_SYNTHESIZED_EVENTS];
    for (const name of everyName) {
      translateSidecarEvent({ event: name }, deps);
    }
    const unhandled = getUnhandledSidecarEventNames();

    // The ignore list stays truthful: every entry really is unhandled.
    for (const ignored of IGNORED_BY_MACHINE) {
      expect(unhandled.has(ignored), `${ignored} is listed as ignored but the switch handles it — remove the stale entry`).toBe(true);
    }

    for (const name of everyName) {
      const handled = !unhandled.has(name);
      expect(
        handled || IGNORED_BY_MACHINE.has(name),
        `${name} reaches the switch's default arm — handle it in translateSidecarEvent (createCanario.ts) or add it to IGNORED_BY_MACHINE with a reason`
      ).toBe(true);
    }
  });
});

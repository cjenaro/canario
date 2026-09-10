// Tests for the sidecar event → state machine mapping in createCanario
import { describe, it, expect, vi, afterEach } from "vitest";
import { createRoot } from "solid-js";
import { createAppMachine, type AppMachine } from "../state/machine";
import { createCanario, type CanarioBridge } from "./createCanario";

type SidecarEvent = Record<string, unknown>;
type CommandHandler = (cmd: { cmd: string } & Record<string, unknown>) => Record<string, unknown>;

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

const defaultHandler: CommandHandler = (cmd) => {
  switch (cmd.cmd) {
    case "is_model_downloaded":
      return { ok: true, data: true };
    case "toggle_recording":
      return { ok: true, data: { recording: true } };
    default:
      return { ok: true };
  }
};

function makeApi(handler: CommandHandler = defaultHandler) {
  let eventCb: ((e: SidecarEvent) => void) | null = null;
  let hotkeyCb: (() => void) | null = null;

  const api = {
    sendCommand: vi.fn(async (cmd: { cmd: string } & Record<string, unknown>) => handler(cmd)),
    onEvent: vi.fn((cb: (e: SidecarEvent) => void) => {
      eventCb = cb;
      return () => {
        eventCb = null;
      };
    }),
    showOverlay: vi.fn(async () => {}),
    hideOverlay: vi.fn(async () => {}),
    showSettings: vi.fn(async () => {}),
    registerShortcut: vi.fn(async () => true),
    unregisterShortcut: vi.fn(async () => {}),
    onHotkey: vi.fn((cb: () => void) => {
      hotkeyCb = cb;
      return () => {
        hotkeyCb = null;
      };
    }),
    getPlatform: vi.fn(async () => ({ platform: "linux", isMac: false, isWindows: false, isLinux: true })),
    getTheme: vi.fn(async () => "dark"),
    setTheme: vi.fn(async () => {}),
    getOnboardingCompleted: vi.fn(async () => true),
    setOnboardingCompleted: vi.fn(async () => {}),
    hideSettings: vi.fn(async () => {}),
    autoPaste: vi.fn(async () => true),
    setAutostart: vi.fn(async () => true),
    updateConfigCache: vi.fn(async () => {}),
    getVersion: vi.fn(async () => ({ electron: "0.0.0", sidecar: "0.0.0", mismatch: false })),
    checkForUpdate: vi.fn(async () => ({ available: false })),
    onUpdateAvailable: vi.fn(() => () => {}),
    onNavigateHistory: vi.fn(() => () => {}),
  };

  return {
    api,
    emit: (e: SidecarEvent) => eventCb?.(e),
    emitHotkey: () => hotkeyCb?.(),
  };
}

async function setup(handler?: CommandHandler): Promise<{
  machine: AppMachine;
  bridge: CanarioBridge;
  mock: ReturnType<typeof makeApi>;
  dispose: () => void;
}> {
  const machine = createAppMachine();
  const mock = makeApi(handler);
  (globalThis as Record<string, unknown>).window = { canario: mock.api };

  let bridge!: CanarioBridge;
  const dispose = createRoot((d) => {
    bridge = createCanario(machine);
    return d;
  });
  // Let onMount run + the initial checkModel() round-trip resolve
  await tick();

  return { machine, bridge, mock, dispose };
}

afterEach(() => {
  delete (globalThis as Record<string, unknown>).window;
});

describe("createCanario event mapping", () => {
  it("checks the model on mount and marks it ready", async () => {
    const { machine, mock } = await setup();
    expect(mock.api.sendCommand).toHaveBeenCalledWith(
      expect.objectContaining({ cmd: "is_model_downloaded" }),
    );
    expect(machine.context().modelReady).toBe(true);
  });

  it("RecordingStarted → recording + overlay shown", async () => {
    const { machine, mock } = await setup();
    mock.emit({ event: "RecordingStarted" });
    expect(machine.state().status).toBe("recording");
    expect(mock.api.showOverlay).toHaveBeenCalled();
  });

  it("RecordingStopped ends the transcribing pipeline and hides the overlay", async () => {
    const { machine, bridge, mock } = await setup();
    mock.emit({ event: "RecordingStarted" });
    await bridge.stopRecording();
    expect(machine.state().status).toBe("transcribing");

    mock.emit({ event: "RecordingStopped" });
    expect(machine.state()).toEqual({ status: "idle", hasModel: true });
    expect(mock.api.hideOverlay).toHaveBeenCalled();
  });

  it("RecordingStopped while still recording is a machine no-op (but hides overlay)", async () => {
    const { machine, mock } = await setup();
    mock.emit({ event: "RecordingStarted" });
    mock.emit({ event: "RecordingStopped" });
    expect(machine.state().status).toBe("recording");
    expect(mock.api.hideOverlay).toHaveBeenCalled();
  });

  it("TranscriptionReady stores text/duration, returns to idle, hides overlay", async () => {
    const { machine, bridge, mock } = await setup();
    mock.emit({ event: "RecordingStarted" });
    await bridge.stopRecording();

    mock.emit({ event: "TranscriptionReady", text: "hello world", duration_secs: 1.5 });
    expect(machine.context().lastTranscription).toBe("hello world");
    expect(machine.context().lastDuration).toBe(1.5);
    expect(machine.state()).toEqual({ status: "idle", hasModel: true });
    expect(mock.api.hideOverlay).toHaveBeenCalled();
  });

  it("RecordingCancelled → straight back to idle, overlay hidden, no transcription stored", async () => {
    const { machine, mock } = await setup();
    mock.emit({ event: "RecordingStarted" });
    expect(machine.state().status).toBe("recording");

    mock.emit({ event: "RecordingCancelled" });
    expect(machine.state()).toEqual({ status: "idle", hasModel: true });
    expect(machine.context().lastTranscription).toBeNull();
    expect(mock.api.hideOverlay).toHaveBeenCalled();
  });

  it("Error stores the message, returns to idle, hides overlay", async () => {
    const { machine, mock } = await setup();
    mock.emit({ event: "RecordingStarted" });
    mock.emit({ event: "Error", message: "mic exploded" });
    expect(machine.context().lastError).toBe("mic exploded");
    expect(machine.state()).toEqual({ status: "idle", hasModel: true });
    expect(mock.api.hideOverlay).toHaveBeenCalled();
  });

  it("ModelDownloadProgress / Complete drive the download flow", async () => {
    const { machine, bridge, mock } = await setup();
    machine.updateContext({ modelReady: false });

    await bridge.downloadModel();
    expect(machine.state()).toEqual({ status: "downloading", progress: 0 });

    mock.emit({ event: "ModelDownloadProgress", progress: 55 });
    expect(machine.state()).toEqual({ status: "downloading", progress: 55 });

    mock.emit({ event: "ModelDownloadComplete" });
    expect(machine.state()).toEqual({ status: "idle", hasModel: true });
    // Readiness is re-derived from the sidecar after the event.
    await tick();
    expect(machine.context().modelReady).toBe(true);
  });

  it("ModelDownloadFailed marks the model not ready and stores the error", async () => {
    const { machine, bridge, mock } = await setup((cmd) => {
      if (cmd.cmd === "is_model_downloaded") return { ok: true, data: false };
      if (cmd.cmd === "download_model") return { ok: true };
      return { ok: true };
    });
    machine.updateContext({ modelReady: false });
    await bridge.downloadModel();

    mock.emit({ event: "ModelDownloadFailed", error: "network down" });
    expect(machine.state()).toEqual({ status: "idle", hasModel: false });
    await tick();
    expect(machine.context().modelReady).toBe(false);
    expect(machine.context().lastError).toBe("network down");
  });

  it("ModelDownloadFailed for another variant keeps the selected ready model ready", async () => {
    // User switched to an already-downloaded variant while a download of a
    // different variant failed — readiness must be re-derived, not zeroed.
    const { machine, bridge, mock } = await setup();
    machine.updateContext({ modelReady: true });
    await bridge.downloadModel();

    mock.emit({ event: "ModelDownloadFailed", error: "network down" });
    await tick();
    expect(machine.context().modelReady).toBe(true);
    expect(machine.context().lastError).toBe("network down");
  });

  it("AudioLevel is a no-op", async () => {
    const { machine, mock } = await setup();
    const before = machine.state();
    mock.emit({ event: "AudioLevel", level: 0.8 });
    expect(machine.state()).toEqual(before);
    expect(mock.api.showOverlay).not.toHaveBeenCalled();
    expect(mock.api.hideOverlay).not.toHaveBeenCalled();
  });

  it("HotkeyTriggered toggles recording via the sidecar command", async () => {
    const { machine, mock } = await setup();
    mock.emit({ event: "HotkeyTriggered" });
    await tick();

    expect(mock.api.sendCommand).toHaveBeenCalledWith(
      expect.objectContaining({ cmd: "toggle_recording" }),
    );
    expect(machine.state().status).toBe("recording");
    expect(mock.api.showOverlay).toHaveBeenCalled();
  });

  it("Electron hotkey (macOS/Windows) also toggles recording", async () => {
    const { machine, mock } = await setup();
    mock.emitHotkey();
    await tick();
    expect(machine.state().status).toBe("recording");
  });
});

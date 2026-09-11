// Tests for createCanario's mount-time reconciliation (canario-dmp.5):
// a settings-window reload resets the machine while core may be
// mid-recording / mid-download — events from before mount are gone, so
// the primitive asks the sidecar for `status` and syncs the machine to
// core truth.
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createRoot } from "solid-js";
import { createAppMachine } from "../state/machine";

function fakeApi(statusData: Record<string, unknown>) {
  return {
    sendCommand: vi.fn((cmd: Record<string, unknown>) => {
      if (cmd.cmd === "status") {
        return Promise.resolve({ id: cmd.id, ok: true, data: statusData });
      }
      return Promise.resolve({ id: cmd.id, ok: true, data: null });
    }),
    onEvent: vi.fn(() => () => {}),
    showOverlay: vi.fn(() => Promise.resolve()),
    hideOverlay: vi.fn(() => Promise.resolve()),
    showSettings: vi.fn(() => Promise.resolve()),
    hideSettings: vi.fn(() => Promise.resolve()),
    registerShortcut: vi.fn(() => Promise.resolve(true)),
    unregisterShortcut: vi.fn(() => Promise.resolve()),
    onHotkey: vi.fn(() => () => {}),
    getPlatform: vi.fn(() =>
      Promise.resolve({ platform: "linux", isMac: false, isWindows: false, isLinux: true })
    ),
    getTheme: vi.fn(() => Promise.resolve("dark")),
    setTheme: vi.fn(() => Promise.resolve()),
    getOnboardingCompleted: vi.fn(() => Promise.resolve(true)),
    setOnboardingCompleted: vi.fn(() => Promise.resolve()),
    autoPaste: vi.fn(() => Promise.resolve(true)),
    setAutostart: vi.fn(() => Promise.resolve(true)),
    updateConfigCache: vi.fn(() => Promise.resolve()),
    getVersion: vi.fn(() =>
      Promise.resolve({
        electron: "0.1.2",
        sidecar: "0.1.2",
        mismatch: false,
        protocol: 1,
        protocolMismatch: false,
      })
    ),
    checkForUpdate: vi.fn(() => Promise.resolve({ available: false })),
    pickFile: vi.fn(() => Promise.resolve(null)),
    onUpdateAvailable: vi.fn(() => () => {}),
    onNavigateHistory: vi.fn(() => () => {}),
  };
}

describe("createCanario mount reconciliation", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("syncs the machine to a mid-flight recording and restores the overlay", async () => {
    const { createCanario } = await import("../primitives/createCanario");
    const api = fakeApi({ recording: true, transcribing: false, downloading: false });
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    let dispose = () => {};
    createRoot((d) => {
      dispose = d;
      machine.updateContext({ modelReady: true });
      createCanario(machine);
    });

    await vi.waitFor(() => {
      expect(machine.state().status).toBe("recording");
    });
    expect(api.showOverlay).toHaveBeenCalled();
    expect(api.sendCommand).toHaveBeenCalledWith(expect.objectContaining({ cmd: "status" }));
    dispose();
  });

  it("syncs the machine to an in-flight download", async () => {
    const { createCanario } = await import("../primitives/createCanario");
    const api = fakeApi({ recording: false, transcribing: false, downloading: true });
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    let dispose = () => {};
    createRoot((d) => {
      dispose = d;
      createCanario(machine);
    });

    await vi.waitFor(() => {
      expect(machine.state().status).toBe("downloading");
    });
    dispose();
  });

  it("leaves an idle backend idle (no spurious transitions)", async () => {
    const { createCanario } = await import("../primitives/createCanario");
    const api = fakeApi({ recording: false, transcribing: false, downloading: false });
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    let dispose = () => {};
    createRoot((d) => {
      dispose = d;
      createCanario(machine);
    });

    await vi.waitFor(() => {
      expect(api.sendCommand).toHaveBeenCalledWith(expect.objectContaining({ cmd: "status" }));
    });
    expect(machine.state().status).toBe("idle");
    expect(api.showOverlay).not.toHaveBeenCalled();
    dispose();
  });

  it("exposes the lifecycle API surface", async () => {
    const { createCanario } = await import("../primitives/createCanario");
    const api = fakeApi({ recording: false, transcribing: false, downloading: false });
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    let dispose = () => {};
    let canarioApi: ReturnType<typeof createCanario> | null = null;
    createRoot((d) => {
      dispose = d;
      canarioApi = createCanario(machine);
    });

    expect(typeof canarioApi!.cancelRecording).toBe("function");
    expect(typeof canarioApi!.cancelDownload).toBe("function");
    expect(typeof canarioApi!.isDownloading).toBe("function");
    expect(typeof canarioApi!.getStatus).toBe("function");

    await canarioApi!.cancelRecording();
    expect(api.sendCommand).toHaveBeenCalledWith(expect.objectContaining({ cmd: "cancel_recording" }));
    await canarioApi!.cancelDownload();
    expect(api.sendCommand).toHaveBeenCalledWith(expect.objectContaining({ cmd: "cancel_download" }));
    dispose();
  });
});

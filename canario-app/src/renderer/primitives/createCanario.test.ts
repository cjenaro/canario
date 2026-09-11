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

// fakeApi, but with a per-command handler: return a response object, a
// promise (for rejection tests), or undefined to fall back to ok:true.
// The mount-time `status` command keeps the fakeApi behavior.
function fakeApiHandling(
  statusData: Record<string, unknown>,
  handle: (cmd: Record<string, unknown>) => Record<string, unknown> | Promise<Record<string, unknown>> | undefined
) {
  return {
    ...fakeApi(statusData),
    sendCommand: vi.fn((cmd: Record<string, unknown>) => {
      if (cmd.cmd === "status") {
        return Promise.resolve({ ok: true, data: statusData });
      }
      const res = handle(cmd);
      return res instanceof Promise ? res : Promise.resolve(res ?? { ok: true, data: null });
    }),
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

  it("notifies transform-fallback subscribers only when the event signals failure (fgm.4)", async () => {
    const { createCanario } = await import("../primitives/createCanario");
    let onEventCb: ((e: Record<string, unknown>) => void) | null = null;
    const api = {
      ...fakeApi({ recording: false, transcribing: false, downloading: false }),
      onEvent: vi.fn((cb: (e: Record<string, unknown>) => void) => {
        onEventCb = cb;
        return () => {};
      }),
    };
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    const seen: Record<string, unknown>[] = [];
    let dispose = () => {};
    let unsub = () => {};
    createRoot((d) => {
      dispose = d;
      const bridge = createCanario(machine);
      unsub = bridge.onTransformFallback((e) => seen.push(e));
    });

    await vi.waitFor(() => expect(onEventCb).not.toBeNull());

    // Clean transcription (today's shape, no failure fields): silent.
    onEventCb!({ event: "TranscriptionReady", text: "words", duration_secs: 2 });
    expect(seen).toHaveLength(0);

    // Failure-flagged transcription (fgm.1 D5d fallback): exactly one
    // notify, carrying the event so the settings toast can decide
    // against the live transform-enabled state.
    onEventCb!({ event: "TranscriptionReady", text: "words", duration_secs: 2, transform_failed: true });
    expect(seen).toHaveLength(1);
    expect(seen[0].transform_failed).toBe(true);

    // Either way the machine completes the pipeline on the canonical
    // event.text — the fallback never changes the state path.
    expect(machine.context().lastTranscription).toBe("words");

    unsub();
    dispose();
  });
});

describe("truthful acks (canario-dmp.7)", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** A mounted bridge over a fake api; `handle` may be re-pointed mid-test. */
  async function mountedBridge(handle: (cmd: Record<string, unknown>) => Record<string, unknown> | Promise<Record<string, unknown>> | undefined) {
    const { createCanario } = await import("../primitives/createCanario");
    const api = fakeApiHandling(
      { recording: false, transcribing: false, downloading: false },
      handle
    );
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    let dispose = () => {};
    let bridge: ReturnType<typeof createCanario> | null = null;
    createRoot((d) => {
      dispose = d;
      bridge = createCanario(machine);
    });
    return { api, machine, bridge: bridge!, dispose };
  }

  it("updateConfig returns true and syncs the main-process cache only on ok", async () => {
    let ok = true;
    const { api, bridge, dispose } = await mountedBridge(
      (cmd) => (cmd.cmd === "update_config" ? { ok, data: null } : undefined)
    );

    await expect(bridge.updateConfig({ auto_paste: false })).resolves.toBe(true);
    expect(api.updateConfigCache).toHaveBeenCalledWith({ auto_paste: false });

    ok = false;
    await expect(bridge.updateConfig({ auto_paste: true })).resolves.toBe(false);
    // A failed write must not fake the cache into main-process decisions.
    expect(api.updateConfigCache).toHaveBeenCalledTimes(1);
    dispose();
  });

  it("updateConfig returns false when the command rejects (sidecar down)", async () => {
    const { api, bridge, dispose } = await mountedBridge((cmd) =>
      cmd.cmd === "update_config"
        ? Promise.reject(new Error("Sidecar not running"))
        : undefined
    );

    await expect(bridge.updateConfig({ theme: "light" })).resolves.toBe(false);
    expect(api.updateConfigCache).not.toHaveBeenCalled();
    dispose();
  });

  it("deleteModel returns the ack and only drops readiness on success", async () => {
    let behavior: "ok" | "rejected" | "failed" = "ok";
    const { machine, bridge, dispose } = await mountedBridge((cmd) => {
      if (cmd.cmd !== "delete_model") return undefined;
      if (behavior === "rejected") return Promise.reject(new Error("crashed"));
      return { ok: behavior === "ok", error: behavior === "failed" ? "in use" : undefined };
    });

    machine.updateContext({ modelReady: true });

    await expect(bridge.deleteModel()).resolves.toBe(true);
    expect(machine.context().modelReady).toBe(false);

    behavior = "failed";
    machine.updateContext({ modelReady: true });
    await expect(bridge.deleteModel()).resolves.toBe(false);
    expect(machine.context().modelReady).toBe(true); // model survived

    behavior = "rejected";
    await expect(bridge.deleteModel()).resolves.toBe(false);
    expect(machine.context().modelReady).toBe(true);
    dispose();
  });

  it("deleteHistory sends the canonical entry_id and returns the ack", async () => {
    let behavior: "ok" | "rejected" | "failed" = "ok";
    const { api, bridge, dispose } = await mountedBridge((cmd) => {
      if (cmd.cmd !== "delete_history") return undefined;
      if (behavior === "rejected") return Promise.reject(new Error("timeout"));
      return { ok: behavior === "ok", error: behavior === "failed" ? "missing entry_id" : undefined };
    });

    await expect(bridge.deleteHistory("abc-123")).resolves.toBe(true);
    expect(api.sendCommand).toHaveBeenCalledWith(
      expect.objectContaining({ cmd: "delete_history", entry_id: "abc-123" })
    );

    behavior = "failed";
    await expect(bridge.deleteHistory("abc-123")).resolves.toBe(false);

    behavior = "rejected";
    await expect(bridge.deleteHistory("abc-123")).resolves.toBe(false);
    dispose();
  });
});

describe("new sidecar events (canario-dmp.9 TranscriptionStarted, canario-dmp.20 ConfigChanged)", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** A mounted bridge whose onEvent callback is captured for firing. */
  async function mountedBridgeWithEvents(
    handle: (cmd: Record<string, unknown>) => Record<string, unknown> | Promise<Record<string, unknown>> | undefined
  ) {
    const { createCanario } = await import("../primitives/createCanario");
    let onEventCb: ((e: Record<string, unknown>) => void) | null = null;
    const api = {
      ...fakeApiHandling({ recording: false, transcribing: false, downloading: false }, handle),
      onEvent: vi.fn((cb: (e: Record<string, unknown>) => void) => {
        onEventCb = cb;
        return () => {};
      }),
    };
    vi.stubGlobal("window", { canario: api });
    const machine = createAppMachine();
    let dispose = () => {};
    let bridge: ReturnType<typeof createCanario> | null = null;
    createRoot((d) => {
      dispose = d;
      bridge = createCanario(machine);
    });
    await vi.waitFor(() => expect(onEventCb).not.toBeNull());
    return { api, machine, bridge: bridge!, dispose, fire: onEventCb! };
  }

  it("TranscriptionStarted moves a recording machine to transcribing — idempotent from transcribing", async () => {
    const { machine, dispose, fire } = await mountedBridgeWithEvents(() => undefined);

    machine.updateContext({ modelReady: true });
    machine.send({ type: "START_RECORDING" });
    expect(machine.state().status).toBe("recording");

    fire({ event: "TranscriptionStarted" });
    expect(machine.state().status).toBe("transcribing");

    // Belt-and-braces only: the stop-response path already got us here,
    // and the machine ignores STOP_RECORDING from `transcribing`.
    fire({ event: "TranscriptionStarted" });
    expect(machine.state().status).toBe("transcribing");
    dispose();
  });

  it("ConfigChanged triggers a get_config pull into context.config", async () => {
    const { api, machine, dispose, fire } = await mountedBridgeWithEvents((cmd) =>
      cmd.cmd === "get_config" ? { ok: true, data: { auto_paste: true } } : undefined
    );

    fire({ event: "ConfigChanged" });

    await vi.waitFor(() => {
      expect(api.sendCommand).toHaveBeenCalledWith(expect.objectContaining({ cmd: "get_config" }));
    });
    await vi.waitFor(() => {
      expect(machine.context().config).toEqual({ auto_paste: true });
    });
    dispose();
  });
});

// Tests for the sidecar host failure paths (canario-dmp.6): a crashed
// process must (a) emit a renderer-visible SidecarCrashed event, (b)
// fail pending commands fast instead of hanging on the 10s timeout,
// (c) fail subsequent commands with the crashed state, and a spawn
// failure must reject startup immediately. A deliberate stop must NOT
// be reported as a crash.
//
// sidecar.ts keeps module-level state, so every test imports a FRESH
// module instance via vi.resetModules() + dynamic import.
import { describe, it, expect, vi, beforeEach } from "vitest";
import { EventEmitter } from "events";

vi.mock("electron", () => ({
  app: { isPackaged: true, getVersion: vi.fn(() => "0.1.2") },
}));

const fakeChild = {
  child: null as FakeChild | null,
};

class FakeChild extends EventEmitter {
  stdin = new EventEmitter() as EventEmitter & { write: ReturnType<typeof vi.fn>; end: () => void; writable: boolean };
  stdout = new EventEmitter();
  stderr = new EventEmitter();
  pid = 4242;

  constructor() {
    super();
    this.stdin.write = vi.fn();
    this.stdin.end = vi.fn();
    this.stdin.writable = true;
  }
}

vi.mock("child_process", () => ({
  spawn: vi.fn(() => {
    const child = new FakeChild();
    fakeChild.child = child;
    return child as unknown as import("child_process").ChildProcess;
  }),
}));

beforeEach(() => {
  vi.resetModules();
  fakeChild.child = null;
  // getSidecarPath() reads process.resourcesPath when packaged — not
  // defined under vitest. join() would throw before spawn().
  (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath =
    "/tmp/canario-test-resources";
});

async function freshSidecar() {
  return import("./sidecar");
}

function emitLine(child: FakeChild, obj: unknown) {
  child.stdout.emit("data", Buffer.from(JSON.stringify(obj) + "\n"));
}

describe("sidecar crash handling", () => {
  it("reaches running after a successful init ping", async () => {
    const sidecar = await freshSidecar();
    const started = sidecar.startSidecar();
    const child = fakeChild.child!;
    emitLine(child, { id: "init", ok: true });
    await expect(started).resolves.toBeUndefined();
    expect(sidecar.getSidecarStatus()).toEqual({ status: "running", exitCode: null });
  });

  it("emits SidecarCrashed, fails pending commands fast, and fail-closes later commands", async () => {
    const sidecar = await freshSidecar();
    const started = sidecar.startSidecar();
    const child = fakeChild.child!;
    emitLine(child, { id: "init", ok: true });
    await started;

    const seen: Record<string, unknown>[] = [];
    sidecar.onSidecarEvent((e) => seen.push(e));

    // In-flight command when the process dies…
    const pending = sidecar.sendCommand({ id: "c1", cmd: "get_config" });
    child.emit("exit", 1);

    await expect(pending).rejects.toThrow(/Sidecar exited \(code 1\)/);
    // …a renderer-visible terminal event…
    expect(seen).toContainEqual({ event: "SidecarCrashed", code: 1 });
    // …and later commands fail fast with the crash state.
    expect(sidecar.getSidecarStatus().status).toBe("crashed");
    await expect(sidecar.sendCommand({ id: "c2", cmd: "ping" })).rejects.toThrow(
      /crashed.*restart Canario/
    );
  });

  it("treats a deliberate stop as stopped, not crashed", async () => {
    const sidecar = await freshSidecar();
    const started = sidecar.startSidecar();
    const child = fakeChild.child!;
    emitLine(child, { id: "init", ok: true });
    await started;

    const seen: Record<string, unknown>[] = [];
    sidecar.onSidecarEvent((e) => seen.push(e));

    sidecar.stopSidecar();
    child.emit("exit", 0);

    expect(sidecar.getSidecarStatus()).toEqual({ status: "stopped", exitCode: 0 });
    expect(seen.filter((e) => e.event === "SidecarCrashed")).toHaveLength(0);
  });

  it("rejects startup immediately when spawning fails (missing binary)", async () => {
    const sidecar = await freshSidecar();
    const started = sidecar.startSidecar();
    const child = fakeChild.child!;
    child.emit("error", new Error("spawn ENOENT"));
    await expect(started).rejects.toThrow(/failed to start: spawn ENOENT/);
    expect(sidecar.getSidecarStatus().status).toBe("crashed");
  });

  it("attaches stream error handlers so EPIPE cannot become an uncaught exception", async () => {
    const sidecar = await freshSidecar();
    const started = sidecar.startSidecar();
    const child = fakeChild.child!;
    emitLine(child, { id: "init", ok: true });
    await started;

    // Simulate an async EPIPE on stdin after the process died: must be
    // swallowed by the attached handler (logged), not thrown.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      expect(() => child.stdin.emit("error", new Error("EPIPE"))).not.toThrow();
      expect(errSpy).toHaveBeenCalled();
    } finally {
      errSpy.mockRestore();
    }
  });
});

// Tests for the sidecar host failure paths (canario-dmp.6): a crashed
// process must (a) emit a renderer-visible SidecarCrashed event, (b)
// fail pending commands fast instead of hanging on the 10s timeout,
// (c) fail subsequent commands with the crashed state, and a spawn
// failure must reject startup immediately. A deliberate stop must NOT
// be reported as a crash.
//
// Plus the stdout fan-out split (canario-dmp.10): responses (lines with
// a string id) route to response listeners and correlate by sendCommand's
// generated ids; events (no id) never match a pending response.
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

/** Every command the host has written to the fake sidecar's stdin. */
function writtenCommands(child: FakeChild): Record<string, unknown>[] {
  return child.stdin.write.mock.calls.map(([line]) => JSON.parse(String(line)));
}

/**
 * Answer the way the real sidecar does: parse the LAST command line the
 * host wrote and respond echoing ITS id — sendCommand generates ids
 * internally (canario-dmp.10), so tests can't guess them.
 */
function respondToLastCommand(child: FakeChild, over: Record<string, unknown> = {}) {
  const req = writtenCommands(child).at(-1)!;
  emitLine(child, { id: req.id, ok: true, ...over });
}

/** A running sidecar: pong the startup ping and await readiness. */
async function runningSidecar() {
  const sidecar = await freshSidecar();
  const started = sidecar.startSidecar();
  const child = fakeChild.child!;
  respondToLastCommand(child); // pong
  await started;
  return { sidecar, child };
}

describe("sidecar crash handling", () => {
  it("reaches running after a successful init ping", async () => {
    const sidecar = await freshSidecar();
    const started = sidecar.startSidecar();
    const child = fakeChild.child!;
    respondToLastCommand(child);
    await expect(started).resolves.toBeUndefined();
    expect(sidecar.getSidecarStatus()).toEqual({ status: "running", exitCode: null });
  });

  it("emits SidecarCrashed, fails pending commands fast, and fail-closes later commands", async () => {
    const { sidecar, child } = await runningSidecar();

    const seen: Record<string, unknown>[] = [];
    sidecar.onSidecarEvent((e) => seen.push(e));

    // In-flight command when the process dies…
    const pending = sidecar.sendCommand({ cmd: "get_config" });
    child.emit("exit", 1);

    await expect(pending).rejects.toThrow(/Sidecar exited \(code 1\)/);
    // …a renderer-visible terminal event…
    expect(seen).toContainEqual({ event: "SidecarCrashed", code: 1 });
    // …and later commands fail fast with the crash state.
    expect(sidecar.getSidecarStatus().status).toBe("crashed");
    await expect(sidecar.sendCommand({ cmd: "ping" })).rejects.toThrow(
      /crashed.*restart Canario/
    );
  });

  it("treats a deliberate stop as stopped, not crashed", async () => {
    const { sidecar, child } = await runningSidecar();

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
    const { child } = await runningSidecar();

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

describe("stdout fan-out: responses vs events (canario-dmp.10)", () => {
  it("routes responses by id — an event never resolves a pending command", async () => {
    const { sidecar, child } = await runningSidecar();
    const seen: Record<string, unknown>[] = [];
    sidecar.onSidecarEvent((e) => seen.push(e));

    const first = sidecar.sendCommand({ cmd: "get_config" });
    const second = sidecar.sendCommand({ cmd: "status" });

    // Two concurrent commands → two distinct generated ids on the wire
    // (the startup ping's cmd is also on stdin — take the last two).
    const sent = writtenCommands(child).slice(-2).map((c) => c.id);
    expect(sent).toHaveLength(2);
    expect(new Set(sent).size).toBe(2);

    // A response for the SECOND command resolves only that command…
    emitLine(child, { id: sent[1], ok: true, data: {} });
    await expect(second).resolves.toEqual({ id: sent[1], ok: true, data: {} });

    // …an id-less event resolves nothing but reaches event consumers…
    emitLine(child, { event: "RecordingStarted" });
    await tick();
    expect(seen).toEqual([{ event: "RecordingStarted" }]);

    // …and a response carrying an unrelated id is dropped, not resolved.
    emitLine(child, { id: "no-such-pending-id", ok: true });
    await tick();
    expect(seen).toHaveLength(1); // responses never reach event consumers

    // The first command still resolves on ITS id.
    emitLine(child, { id: sent[0], ok: true, data: { config: true } });
    await expect(first).resolves.toEqual({ id: sent[0], ok: true, data: { config: true } });
    expect(seen).toHaveLength(1);
  });

  it("overrides caller-provided ids and keeps every generated id unique", async () => {
    const { sidecar, child } = await runningSidecar();

    const a = sidecar.sendCommand({ id: "collide", cmd: "ping" });
    const b = sidecar.sendCommand({ id: "collide", cmd: "ping" });

    const sent = writtenCommands(child).slice(-2);
    // Caller ids are ignored — a hand-written literal can no longer make
    // two in-flight commands resolve each other's responses.
    expect(sent.map((c) => c.id)).not.toContain("collide");
    expect(new Set(sent.map((c) => c.id)).size).toBe(2);

    emitLine(child, { id: sent[1].id, ok: true });
    await expect(b).resolves.toMatchObject({ ok: true });
    emitLine(child, { id: sent[0].id, ok: true });
    await expect(a).resolves.toMatchObject({ ok: true });
  });

  it("notifies onCommandResponse observers with the command as sent", async () => {
    const { sidecar, child } = await runningSidecar();
    const seenCmds: Record<string, unknown>[] = [];
    sidecar.onCommandResponse((cmd, res) => seenCmds.push({ ...cmd, ok: res.ok }));

    const done = sidecar.sendCommand({ cmd: "stop_recording" });
    const sent = writtenCommands(child).at(-1)!;
    emitLine(child, { id: sent.id, ok: true, data: { recording: false } });
    await expect(done).resolves.toMatchObject({ ok: true });

    // The observer sees the wire command (generated id included), and
    // only for commands that actually resolved.
    expect(seenCmds).toEqual([
      { cmd: "stop_recording", id: sent.id, ok: true },
    ]);
  });
});

/** Let queued microtasks (listener callbacks → promise resolutions) run. */
function tick() {
  return new Promise((r) => setTimeout(r, 0));
}

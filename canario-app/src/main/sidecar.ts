// Sidecar manager — spawn + manage the Rust canario-electron process
import { spawn, ChildProcess } from "child_process";
import { join } from "path";
import { app } from "electron";

let sidecar: ChildProcess | null = null;
let eventListeners: Set<(event: Record<string, unknown>) => void> = new Set();
let commandResponseListeners: Set<(cmd: Record<string, unknown>, res: Record<string, unknown>) => void> = new Set();
let buffer = "";

/** Lifecycle of the backend process (canario-dmp.6). */
export type SidecarStatus = "stopped" | "starting" | "running" | "crashed";

let status: SidecarStatus = "stopped";
let lastExitCode: number | string | null = null;

/** In-flight commands, so a crash can fail them fast instead of hanging 10s. */
interface PendingCommand {
  label: string;
  timer: ReturnType<typeof setTimeout>;
  reject: (err: Error) => void;
}
const pendingCommands = new Map<string, PendingCommand>();

/** Rejects the startup promise when the process fails to spawn at all. */
let startupFailure: ((err: Error) => void) | null = null;

export function getSidecarStatus(): { status: SidecarStatus; exitCode: number | string | null } {
  return { status, exitCode: lastExitCode };
}

function getSidecarPath(): string {
  const isDev = !app.isPackaged;
  const ext = process.platform === "win32" ? ".exe" : "";
  const binName = `canario-electron${ext}`;

  if (isDev) {
    // In dev, use the debug-built binary from the Rust target dir
    // out/main/ → canario-app/ → canario/ → target/debug/
    return join(__dirname, `../../../target/debug/${binName}`);
  }
  // In production, bundled alongside the app
  return join(process.resourcesPath, "sidecar", binName);
}

function emitToListeners(event: Record<string, unknown>) {
  for (const listener of eventListeners) {
    try {
      listener(event);
    } catch (err) {
      console.error("Sidecar event listener error:", err);
    }
  }
}

export async function startSidecar(): Promise<void> {
  const sidecarPath = getSidecarPath();
  status = "starting";

  sidecar = spawn(sidecarPath, [], {
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, RUST_LOG: "info" },
  });

  // An async stream error (e.g. EPIPE when the process dies mid-write)
  // would be an uncaught exception — handle every stream explicitly
  // (canario-dmp.6).
  sidecar.stdin?.on("error", (err) => {
    console.error("[sidecar] stdin error:", err);
  });
  sidecar.stdout?.on("error", (err) => {
    console.error("[sidecar] stdout error:", err);
  });
  sidecar.stderr?.on("error", (err) => {
    console.error("[sidecar] stderr error:", err);
  });

  sidecar.stdout?.on("data", (data: Buffer) => {
    buffer += data.toString();
    const lines = buffer.split("\n");
    buffer = lines.pop() || ""; // keep incomplete line in buffer

    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const event = JSON.parse(trimmed);
        emitToListeners(event);
      } catch {
        console.error("Failed to parse sidecar event:", trimmed);
      }
    }
  });

  sidecar.stderr?.on("data", (data: Buffer) => {
    console.error("[sidecar]", data.toString().trim());
  });

  sidecar.on("error", (err) => {
    // Spawn failure (missing/unexecutable binary) lands here. The init
    // ping would otherwise hang for its full 5s timeout before the
    // whenReady .catch can show the failure dialog.
    console.error("Sidecar failed to start:", err);
    sidecar = null;
    status = "crashed";
    lastExitCode = null;
    failPendingCommands(`Sidecar failed to start: ${err.message}`);
    startupFailure?.(new Error(`Sidecar failed to start: ${err.message}`));
    startupFailure = null;
  });

  sidecar.on("exit", (code) => {
    console.log(`Sidecar exited with code ${code}`);
    lastExitCode = code;
    // stopSidecar() nulls `sidecar` BEFORE the process exits, so a
    // still-set reference means the process died on its own.
    const crashed = sidecar !== null;
    sidecar = null;
    status = crashed ? "crashed" : "stopped";
    failPendingCommands(
      `Sidecar exited (code ${code ?? "signal"})${crashed ? "" : " during shutdown"}`
    );
    if (crashed) {
      // Renderer-visible terminal event: without it the machine would
      // wait forever for a TranscriptionReady that can never arrive
      // (canario-dmp.6 zombie-app fix).
      emitToListeners({ event: "SidecarCrashed", code });
    }
  });

  // Wait for sidecar to be ready (ping/pong)
  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => {
      eventListeners.delete(onPong);
      startupFailure = null;
      reject(new Error("Sidecar ping timeout"));
    }, 5000);

    startupFailure = (err) => {
      clearTimeout(timeout);
      eventListeners.delete(onPong);
      reject(err);
    };

    function onPong(event: Record<string, unknown>) {
      if (event.id === "init" && event.ok) {
        clearTimeout(timeout);
        eventListeners.delete(onPong);
        startupFailure = null;
        status = "running";
        resolve();
      }
    }

    eventListeners.add(onPong);
    // Fire-and-forget by design: the outer promise owns error reporting
    // (startupFailure + timeout below). Without this .catch, a spawn
    // failure makes failPendingCommands reject the ping promise with no
    // handler attached — an unhandled rejection (fails CI's vitest run).
    sendCommand({ id: "init", cmd: "ping" }).catch(() => {});
  });
}

/** Reject every in-flight command — used when the process goes away. */
function failPendingCommands(message: string): void {
  for (const [, pending] of pendingCommands) {
    clearTimeout(pending.timer);
    pending.reject(new Error(message));
  }
  pendingCommands.clear();
}

export function stopSidecar(): void {
  if (!sidecar) return;

  const proc = sidecar;
  sidecar = null;
  status = "stopped";

  // Ask nicely first
  try {
    proc.stdin?.write(JSON.stringify({ id: "exit", cmd: "shutdown" }) + "\n");
    proc.stdin?.end();
  } catch { /* already closed */ }

  // Force-kill after 500ms
  setTimeout(() => {
    if (proc.pid) {
      try { process.kill(proc.pid, "SIGKILL"); } catch { /* already dead */ }
    }
  }, 500);
}

export function sendCommand(cmd: Record<string, unknown>): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    // Fail fast with the process state instead of a generic message —
    // callers (and users) can tell a crash from "never started"
    // (canario-dmp.6).
    if (status === "crashed") {
      reject(new Error(`Sidecar crashed (code ${lastExitCode ?? "unknown"}) — restart Canario`));
      return;
    }
    if (!sidecar?.stdin?.writable) {
      reject(new Error("Sidecar not running"));
      return;
    }

    const id = cmd.id as string;

    const timeout = setTimeout(() => {
      eventListeners.delete(onResponse);
      pendingCommands.delete(id);
      reject(new Error(`Command timeout: ${cmd.cmd}`));
    }, 10000);

    pendingCommands.set(id, {
      label: String(cmd.cmd ?? "unknown"),
      timer: timeout,
      reject,
    });

    function onResponse(event: Record<string, unknown>) {
      if (event.id === id) {
        clearTimeout(timeout);
        eventListeners.delete(onResponse);
        pendingCommands.delete(id);
        for (const listener of commandResponseListeners) {
          try {
            listener(cmd, event);
          } catch (err) {
            console.error("Command response listener error:", err);
          }
        }
        resolve(event);
      }
    }

    eventListeners.add(onResponse);
    const json = JSON.stringify(cmd) + "\n";
    try {
      sidecar.stdin!.write(json);
    } catch (err) {
      // Sync write failure on a just-died stream (races the exit event).
      clearTimeout(timeout);
      eventListeners.delete(onResponse);
      pendingCommands.delete(id);
      reject(err instanceof Error ? err : new Error(String(err)));
    }
  });
}

export function onSidecarEvent(callback: (event: Record<string, unknown>) => void): void {
  eventListeners.add(callback);
}

/** Observe every sidecar command response (e.g. to detect a recording stop). */
export function onCommandResponse(
  callback: (cmd: Record<string, unknown>, res: Record<string, unknown>) => void
): void {
  commandResponseListeners.add(callback);
}

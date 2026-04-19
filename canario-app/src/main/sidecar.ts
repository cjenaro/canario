// Sidecar manager — spawn + manage the Rust canario-electron process
import { spawn, ChildProcess } from "child_process";
import { join } from "path";
import { app } from "electron";

let sidecar: ChildProcess | null = null;
let eventListeners: Set<(event: Record<string, unknown>) => void> = new Set();
let buffer = "";

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

  sidecar = spawn(sidecarPath, [], {
    stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, RUST_LOG: "info" },
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

  sidecar.on("exit", (code) => {
    console.log(`Sidecar exited with code ${code}`);
    sidecar = null;
  });

  sidecar.on("error", (err) => {
    console.error("Sidecar error:", err);
  });

  // Wait for sidecar to be ready (ping/pong)
  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => {
      eventListeners.delete(onPong);
      reject(new Error("Sidecar ping timeout"));
    }, 5000);

    function onPong(event: Record<string, unknown>) {
      if (event.id === "init" && event.ok) {
        clearTimeout(timeout);
        eventListeners.delete(onPong);
        resolve();
      }
    }

    eventListeners.add(onPong);
    sendCommand({ id: "init", cmd: "ping" });
  });
}

export function stopSidecar(): void {
  if (!sidecar) return;

  const proc = sidecar;
  sidecar = null;

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
    if (!sidecar?.stdin?.writable) {
      reject(new Error("Sidecar not running"));
      return;
    }

    const id = cmd.id as string;

    const timeout = setTimeout(() => {
      eventListeners.delete(onResponse);
      reject(new Error(`Command timeout: ${cmd.cmd}`));
    }, 10000);

    function onResponse(event: Record<string, unknown>) {
      if (event.id === id) {
        clearTimeout(timeout);
        eventListeners.delete(onResponse);
        resolve(event);
      }
    }

    eventListeners.add(onResponse);
    const json = JSON.stringify(cmd) + "\n";
    sidecar.stdin!.write(json);
  });
}

export function onSidecarEvent(callback: (event: Record<string, unknown>) => void): void {
  eventListeners.add(callback);
}

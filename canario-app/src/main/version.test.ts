// Tests for the sidecar version/protocol handshake (canario-dmp.4):
// a protocol mismatch (different number, or a sidecar that predates
// the handshake entirely) must set protocolMismatch so the renderer
// can show its persistent warning instead of silently misbehaving on
// drifted commands/events/response shapes.
//
// version.ts keeps module-level state (sidecarVersion, protocolMismatch,
// …), so every test imports a FRESH module instance via
// vi.resetModules() + dynamic import.
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("electron", () => ({
  app: {
    getVersion: vi.fn(() => "0.1.2"),
  },
}));

vi.mock("./sidecar.js", () => ({
  sendCommand: vi.fn(),
}));

import { sendCommand } from "./sidecar.js";

const sendMock = vi.mocked(sendCommand);

function pingResponse(payload: Record<string, unknown>) {
  return { id: "version-check", ok: true, data: payload };
}

beforeEach(() => {
  sendMock.mockReset();
  vi.resetModules();
});

async function freshVersionModule() {
  return import("./version");
}

describe("checkVersion protocol handshake", () => {
  it("accepts a matching protocol and version", async () => {
    const { PROTOCOL_VERSION, checkVersion, getVersionInfo, versionWarningText } =
      await freshVersionModule();
    sendMock.mockResolvedValue(
      pingResponse({ pong: true, version: "0.1.2", protocol: PROTOCOL_VERSION })
    );
    await checkVersion();

    const info = getVersionInfo();
    expect(info.protocol).toBe(PROTOCOL_VERSION);
    expect(info.protocolMismatch).toBe(false);
    expect(info.mismatch).toBe(false);
    expect(versionWarningText()).toBeNull();
  });

  it("flags a different protocol number as a mismatch", async () => {
    const { PROTOCOL_VERSION, checkVersion, getVersionInfo, versionWarningText } =
      await freshVersionModule();
    sendMock.mockResolvedValue(
      pingResponse({ pong: true, version: "0.1.2", protocol: PROTOCOL_VERSION + 1 })
    );
    await checkVersion();

    const info = getVersionInfo();
    expect(info.protocol).toBe(PROTOCOL_VERSION + 1);
    expect(info.protocolMismatch).toBe(true);
    expect(versionWarningText()).toContain("Protocol mismatch");
    expect(versionWarningText()).toContain(String(PROTOCOL_VERSION + 1));
  });

  it("treats a missing protocol number as incompatible", async () => {
    // A sidecar without the field predates the handshake — it could be
    // arbitrarily old, so never guess compatibility.
    const { checkVersion, getVersionInfo, versionWarningText } = await freshVersionModule();
    sendMock.mockResolvedValue(pingResponse({ pong: true, version: "0.1.2" }));
    await checkVersion();

    const info = getVersionInfo();
    expect(info.protocol).toBeNull();
    expect(info.protocolMismatch).toBe(true);
    expect(versionWarningText()).toContain("unknown");
  });

  it("still reports a crate-version mismatch independently", async () => {
    const { PROTOCOL_VERSION, checkVersion, getVersionInfo, versionWarningText } =
      await freshVersionModule();
    sendMock.mockResolvedValue(
      pingResponse({ pong: true, version: "0.1.1", protocol: PROTOCOL_VERSION })
    );
    await checkVersion();

    const info = getVersionInfo();
    expect(info.mismatch).toBe(true);
    expect(info.protocolMismatch).toBe(false);
    expect(versionWarningText()).toContain("0.1.1");
    expect(versionWarningText()).toContain("0.1.2");
  });

  it("a failed ping leaves everything unknown without throwing", async () => {
    const { checkVersion, getVersionInfo } = await freshVersionModule();
    sendMock.mockRejectedValue(new Error("Sidecar not running"));
    await expect(checkVersion()).resolves.toBeUndefined();

    const info = getVersionInfo();
    expect(info.sidecar).toBeNull();
    expect(info.protocol).toBeNull();
    expect(info.protocolMismatch).toBe(false);
  });
});

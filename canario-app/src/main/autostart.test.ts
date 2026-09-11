// Tests for autostart delegation (issue canario-dmp.17): Linux no longer
// writes ~/.config/autostart/canario.desktop from the Electron process —
// the sidecar's set_autostart owns the desktop entry AND persists
// config.autostart on success. macOS/Windows still use
// app.setLoginItemSettings for the OS entry but persist the flag through
// the sidecar's update_config. Any failure must surface as `false` so the
// renderer's toast/revert can react.
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("electron", () => ({
  app: {
    isPackaged: false,
    getPath: vi.fn(),
    setLoginItemSettings: vi.fn(),
  },
}));

vi.mock("./sidecar.js", () => ({
  sendCommand: vi.fn(),
}));

import { app } from "electron";
import { sendCommand } from "./sidecar.js";
import { setAutostart } from "./autostart";

const sendCommandMock = vi.mocked(sendCommand);
const getPathMock = vi.mocked(app.getPath);
const setLoginItemSettingsMock = vi.mocked(app.setLoginItemSettings);

let originalPlatform: string;

function stubPlatform(platform: string) {
  originalPlatform = process.platform;
  Object.defineProperty(process, "platform", {
    value: platform,
    configurable: true,
  });
}

// `app.isPackaged` is readonly in Electron's typings, but this is the
// plain mock object from vi.mock above — mutate it the defineProperty way.
function stubPackaged(packaged: boolean) {
  Object.defineProperty(app, "isPackaged", { value: packaged, configurable: true });
}

const ok = (over: Record<string, unknown> = {}) => ({ ok: true, data: {}, ...over });

beforeEach(() => {
  vi.resetAllMocks();
  stubPackaged(false);
});

afterEach(() => {
  if (originalPlatform !== undefined) {
    stubPlatform(originalPlatform);
    originalPlatform = undefined as unknown as string;
  }
});

describe("setAutostart on Linux", () => {
  it("delegates to set_autostart with the dev Electron binary as exec and returns true on ok", async () => {
    stubPlatform("linux");
    sendCommandMock.mockResolvedValue(ok({ data: { enabled: true } }));

    await expect(setAutostart(true)).resolves.toBe(true);

    expect(sendCommand).toHaveBeenCalledOnce();
    expect(sendCommandMock.mock.calls[0][0]).toEqual({
      id: expect.stringMatching(/^autostart-\d+$/),
      cmd: "set_autostart",
      enabled: true,
      exec: process.execPath,
    });
    // The desktop file is the sidecar's business now — no Electron login API.
    expect(setLoginItemSettingsMock).not.toHaveBeenCalled();
  });

  it("uses app.getPath(\"exe\") as exec when packaged", async () => {
    stubPlatform("linux");
    stubPackaged(true);
    getPathMock.mockReturnValue("/opt/Canario/canario");
    sendCommandMock.mockResolvedValue(ok({ data: { enabled: true } }));

    await expect(setAutostart(true)).resolves.toBe(true);

    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "set_autostart",
      exec: "/opt/Canario/canario",
    });
  });

  it("sends enabled:false when disabling", async () => {
    stubPlatform("linux");
    sendCommandMock.mockResolvedValue(ok({ data: { enabled: false } }));

    await expect(setAutostart(false)).resolves.toBe(true);

    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "set_autostart",
      enabled: false,
    });
  });

  it("returns false when the sidecar responds ok:false", async () => {
    stubPlatform("linux");
    sendCommandMock.mockResolvedValue({ id: "autostart-1", ok: false, error: "not supported" });

    await expect(setAutostart(true)).resolves.toBe(false);
  });

  it("returns false when sendCommand rejects (sidecar down / timeout)", async () => {
    stubPlatform("linux");
    sendCommandMock.mockRejectedValue(new Error("Sidecar not running"));

    await expect(setAutostart(true)).resolves.toBe(false);
    await expect(setAutostart(false)).resolves.toBe(false);
  });

  it("issues distinct ids per command while responses may be pending", async () => {
    stubPlatform("linux");
    sendCommandMock.mockResolvedValue(ok());

    await setAutostart(true);
    await setAutostart(false);

    const ids = sendCommandMock.mock.calls.map(([cmd]) => (cmd as { id: string }).id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("setAutostart on macOS/Windows", () => {
  it("sets the login item and persists the flag via update_config", async () => {
    stubPlatform("darwin");
    sendCommandMock.mockResolvedValue(ok());

    await expect(setAutostart(true)).resolves.toBe(true);

    expect(setLoginItemSettingsMock).toHaveBeenCalledOnce();
    expect(setLoginItemSettingsMock).toHaveBeenCalledWith({ openAtLogin: true });
    expect(sendCommand).toHaveBeenCalledOnce();
    expect(sendCommandMock.mock.calls[0][0]).toEqual({
      id: expect.stringMatching(/^autostart-\d+$/),
      cmd: "update_config",
      config: { autostart: true },
    });
  });

  it("uses the same shape on Windows", async () => {
    stubPlatform("win32");
    sendCommandMock.mockResolvedValue(ok());

    await expect(setAutostart(true)).resolves.toBe(true);

    expect(setLoginItemSettingsMock).toHaveBeenCalledWith({ openAtLogin: true });
    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "update_config",
      config: { autostart: true },
    });
  });

  it("disables the login item and persists autostart:false", async () => {
    stubPlatform("darwin");
    sendCommandMock.mockResolvedValue(ok());

    await expect(setAutostart(false)).resolves.toBe(true);

    expect(setLoginItemSettingsMock).toHaveBeenCalledWith({ openAtLogin: false });
    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "update_config",
      config: { autostart: false },
    });
  });

  it("returns false when the flag persist is rejected (entry changed, flag did not)", async () => {
    stubPlatform("darwin");
    sendCommandMock.mockResolvedValue({ id: "autostart-1", ok: false, error: "io error" });

    await expect(setAutostart(true)).resolves.toBe(false);
    // The OS entry was still attempted before the flag write failed.
    expect(setLoginItemSettingsMock).toHaveBeenCalledOnce();
  });

  it("returns false when sendCommand rejects after setting the login item", async () => {
    stubPlatform("darwin");
    sendCommandMock.mockRejectedValue(new Error("Command timeout: update_config"));

    await expect(setAutostart(true)).resolves.toBe(false);
    expect(setLoginItemSettingsMock).toHaveBeenCalledOnce();
  });

  it("returns false before touching the config when the login-item call throws", async () => {
    stubPlatform("darwin");
    setLoginItemSettingsMock.mockImplementation(() => {
      throw new Error("registry locked");
    });

    await expect(setAutostart(true)).resolves.toBe(false);
    expect(sendCommandMock).not.toHaveBeenCalled();
  });
});

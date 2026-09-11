// Tests for the transform credential vault (canario-fgm.2, decision
// D2): the API key persists via safeStorage (never config.json, never
// echoed to the renderer), reaches the sidecar's memory via
// set_transform_credential, emptying the field clears BOTH the file
// and the sidecar copy, and every failure path resolves with
// { ok: false } instead of throwing (no unhandled rejections — the
// sidecar.test.ts lesson).
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { mkdtempSync, rmSync, existsSync, readFileSync, statSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";

// Deterministic fake: "enc:<plaintext>" stands in for the OS-keyring
// blob; anything else fails to decrypt (the cross-keyring case).
vi.mock("electron", () => ({
  app: {
    getPath: vi.fn(),
  },
  safeStorage: {
    isEncryptionAvailable: vi.fn(() => true),
    setUsePlainTextEncryption: vi.fn(),
    encryptString: vi.fn((plain: string) => Buffer.from(`enc:${plain}`, "utf8")),
    decryptString: vi.fn((blob: Buffer) => {
      const text = blob.toString("utf8");
      if (!text.startsWith("enc:")) throw new Error("oscrypt error: decryption failed");
      return text.slice("enc:".length);
    }),
  },
}));

vi.mock("./sidecar.js", () => ({
  sendCommand: vi.fn(),
}));

import { app, safeStorage } from "electron";
import { sendCommand } from "./sidecar.js";
import {
  clearTransformCredential,
  initTransformCredential,
  loadTransformCredential,
  saveTransformCredential,
} from "./transformCredential";

const sendCommandMock = vi.mocked(sendCommand);
const getPathMock = vi.mocked(app.getPath);
const isEncryptionAvailableMock = vi.mocked(safeStorage.isEncryptionAvailable);
const encryptMock = vi.mocked(safeStorage.encryptString);
const decryptMock = vi.mocked(safeStorage.decryptString);
const setUsePlainTextMock = vi.mocked(safeStorage.setUsePlainTextEncryption);

const okStored = (stored: boolean) => ({ ok: true, data: { stored } });

let userDataDir: string;
let originalPlatform: string | undefined;

function stubPlatform(platform: string) {
  originalPlatform = process.platform;
  Object.defineProperty(process, "platform", { value: platform, configurable: true });
}

function keyPath(): string {
  return join(userDataDir, "transform-key.bin");
}

beforeEach(() => {
  // resetAllMocks (not clearAllMocks): tests below override factory
  // implementations (encrypt throws, reject values) and Vitest's
  // resetAllMocks restores the originals passed to vi.fn(...) while
  // wiping per-test overrides.
  vi.resetAllMocks();
  isEncryptionAvailableMock.mockReturnValue(true);
  sendCommandMock.mockResolvedValue(okStored(true));
  userDataDir = mkdtempSync(join(tmpdir(), "canario-transform-"));
  getPathMock.mockReturnValue(userDataDir);
});

afterEach(() => {
  rmSync(userDataDir, { recursive: true, force: true });
  if (originalPlatform !== undefined) {
    stubPlatform(originalPlatform);
    originalPlatform = undefined;
  }
});

describe("saveTransformCredential", () => {
  it("persists an encrypted blob with 0600 perms and pushes the trimmed key to the sidecar", async () => {
    await expect(saveTransformCredential("  sk-live-123  ")).resolves.toEqual({
      ok: true,
      stored: true,
    });

    expect(encryptMock).toHaveBeenCalledOnce();
    expect(encryptMock).toHaveBeenCalledWith("sk-live-123");
    expect(existsSync(keyPath())).toBe(true);
    expect(readFileSync(keyPath(), "utf8")).toBe("enc:sk-live-123");
    expect(statSync(keyPath()).mode & 0o777).toBe(0o600);

    expect(sendCommand).toHaveBeenCalledOnce();
    expect(sendCommandMock.mock.calls[0][0]).toEqual({
      id: expect.stringMatching(/^transform-credential-\d+$/),
      cmd: "set_transform_credential",
      key: "sk-live-123",
    });
  });

  it("empty and whitespace-only values clear (file + sidecar memory)", async () => {
    writeFileSync(keyPath(), Buffer.from("enc:sk-old", "utf8"));
    sendCommandMock.mockResolvedValue(okStored(false));

    for (const empty of ["", "   "]) {
      await expect(saveTransformCredential(empty)).resolves.toEqual({ ok: true, stored: false });
      expect(existsSync(keyPath())).toBe(false);
      expect(sendCommandMock).toHaveBeenLastCalledWith({
        id: expect.stringMatching(/^transform-credential-\d+$/),
        cmd: "set_transform_credential",
        key: null,
      });
      expect(encryptMock).not.toHaveBeenCalled();
    }
  });

  it("clearing without a stored file still syncs the sidecar (idempotent)", async () => {
    sendCommandMock.mockResolvedValue(okStored(false));

    await expect(clearTransformCredential()).resolves.toEqual({ ok: true, stored: false });

    expect(existsSync(keyPath())).toBe(false);
    expect(sendCommand).toHaveBeenCalledOnce();
    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({ cmd: "set_transform_credential", key: null });
  });

  it("resolves ok:false (no throw) when safeStorage is unavailable", async () => {
    isEncryptionAvailableMock.mockReturnValue(false);

    await expect(saveTransformCredential("sk-x")).resolves.toMatchObject({ ok: false, stored: false });

    expect(encryptMock).not.toHaveBeenCalled();
    expect(existsSync(keyPath())).toBe(false);
    expect(sendCommand).not.toHaveBeenCalled();
  });

  it("resolves ok:false when encryption throws", async () => {
    encryptMock.mockImplementation(() => {
      throw new Error("keyring locked");
    });

    await expect(saveTransformCredential("sk-x")).resolves.toMatchObject({
      ok: false,
      error: expect.stringContaining("keyring locked"),
    });
    expect(existsSync(keyPath())).toBe(false);
  });

  it("resolves ok:false when the key file cannot be written", async () => {
    getPathMock.mockImplementation(() => {
      throw new Error("userData unavailable");
    });

    await expect(saveTransformCredential("sk-x")).resolves.toMatchObject({
      ok: false,
      error: expect.stringContaining("userData unavailable"),
    });
    expect(sendCommand).not.toHaveBeenCalled();
  });

  it("keeps the persisted key (ok:false, no throw) when the sidecar push rejects", async () => {
    sendCommandMock.mockRejectedValue(new Error("Sidecar not running"));

    await expect(saveTransformCredential("sk-x")).resolves.toMatchObject({
      ok: false,
      stored: false,
      error: "Sidecar not running",
    });
    // Persisted BEFORE pushing: the next boot's initTransformCredential
    // delivers it.
    expect(existsSync(keyPath())).toBe(true);
  });

  it("resolves ok:false when the sidecar rejects the command", async () => {
    sendCommandMock.mockResolvedValue({ ok: false, error: "boom" });

    await expect(saveTransformCredential("sk-x")).resolves.toMatchObject({
      ok: false,
      error: expect.stringContaining("boom"),
    });
  });

  it("issues distinct command ids while responses may be pending", async () => {
    await saveTransformCredential("sk-a");
    await saveTransformCredential("sk-b");

    const ids = sendCommandMock.mock.calls.map(([cmd]) => (cmd as { id: string }).id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("loadTransformCredential", () => {
  it("round-trips a stored blob", () => {
    writeFileSync(keyPath(), Buffer.from("enc:sk-round-trip", "utf8"));

    expect(loadTransformCredential()).toBe("sk-round-trip");
    // No sidecar traffic on the read path.
    expect(sendCommand).not.toHaveBeenCalled();
  });

  it("returns null when no file exists", () => {
    expect(loadTransformCredential()).toBeNull();
  });

  it("returns null for an undecryptable blob (foreign keyring / corruption)", () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      writeFileSync(keyPath(), Buffer.from("garbage-not-a-blob", "utf8"));

      expect(loadTransformCredential()).toBeNull();
      expect(warnSpy).toHaveBeenCalled();
    } finally {
      warnSpy.mockRestore();
    }
  });

  it("returns null when safeStorage is unavailable", () => {
    isEncryptionAvailableMock.mockReturnValue(false);
    writeFileSync(keyPath(), Buffer.from("enc:sk-unreachable", "utf8"));

    expect(loadTransformCredential()).toBeNull();
  });

  it("trims to null for a whitespace-only decrypted value", () => {
    writeFileSync(keyPath(), Buffer.from("enc:   ", "utf8"));

    expect(loadTransformCredential()).toBeNull();
  });
});

describe("initTransformCredential", () => {
  it("pushes the decrypted key at boot", async () => {
    writeFileSync(keyPath(), Buffer.from("enc:sk-boot", "utf8"));

    await expect(initTransformCredential()).resolves.toBeUndefined();

    expect(sendCommand).toHaveBeenCalledOnce();
    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "set_transform_credential",
      key: "sk-boot",
    });
  });

  it("pushes key:null when nothing is stored", async () => {
    sendCommandMock.mockResolvedValue(okStored(false));

    await expect(initTransformCredential()).resolves.toBeUndefined();

    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "set_transform_credential",
      key: null,
    });
  });

  it("pushes key:null when the stored blob is unreadable", async () => {
    sendCommandMock.mockResolvedValue(okStored(false));
    writeFileSync(keyPath(), Buffer.from("garbage", "utf8"));

    await expect(initTransformCredential()).resolves.toBeUndefined();

    expect(sendCommandMock.mock.calls[0][0]).toMatchObject({
      cmd: "set_transform_credential",
      key: null,
    });
  });

  it("never rejects when the push fails (startup must not die on it)", async () => {
    sendCommandMock.mockRejectedValue(new Error("Command timeout"));

    await expect(initTransformCredential()).resolves.toBeUndefined();
  });
});

describe("Linux basic_text fallback", () => {
  it("opts into plaintext encryption on Linux", async () => {
    stubPlatform("linux");

    await saveTransformCredential("sk-linux");

    expect(setUsePlainTextMock).toHaveBeenCalledWith(true);
  });

  it("loads also opt in on Linux (boot path)", () => {
    stubPlatform("linux");

    loadTransformCredential();

    expect(setUsePlainTextMock).toHaveBeenCalledWith(true);
  });

  it("does not touch the flag on other platforms", async () => {
    stubPlatform("darwin");

    await saveTransformCredential("sk-mac");

    expect(setUsePlainTextMock).not.toHaveBeenCalled();
  });
});

// Tests for the Linux auto-paste strategy (canario-cy0): clipboard +
// synthesized Ctrl+V first — but only when a read-back verified the
// fresh text (canario-fhm stale-clipboard guard) — with char-by-char
// typing as the fallback. The macOS/Windows arm (canario-ubb) routes
// through the sidecar's native paste command.
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("electron", () => ({
  clipboard: { readText: vi.fn(), writeText: vi.fn() },
  systemPreferences: {},
  dialog: {},
  BrowserWindow: {},
}));

const sendCommandMock = vi.fn();
vi.mock("./sidecar.js", () => ({ sendCommand: (...a: unknown[]) => sendCommandMock(...a) }));

import { autoPasteText, clipboardHoldsText, linuxPastePlan, linuxPasteStepArgs } from "./autoPaste";

const { readText, writeText } = vi.mocked(await import("electron")).clipboard as unknown as {
  readText: ReturnType<typeof vi.fn>;
  writeText: ReturnType<typeof vi.fn>;
};

describe("linuxPastePlan", () => {
  it("puts verified-clipboard Ctrl+V before typing on Wayland", () => {
    expect(linuxPastePlan(true, true)).toEqual([
      { kind: "ctrl_v", tool: "ydotool" },
      { kind: "type", tool: "wtype" },
      { kind: "type", tool: "ydotool" },
    ]);
  });

  it("puts verified-clipboard Ctrl+V before typing on X11", () => {
    expect(linuxPastePlan(true, false)).toEqual([
      { kind: "ctrl_v", tool: "xdotool" },
      { kind: "ctrl_v", tool: "ydotool" },
      { kind: "type", tool: "xdotool" },
      { kind: "type", tool: "ydotool" },
    ]);
  });

  it("skips the paste shortcut entirely when the clipboard is unverified", () => {
    // A Ctrl+V into an unverified clipboard would paste stale content
    // (canario-fhm) — typing is the only safe strategy.
    expect(linuxPastePlan(false, true)).toEqual([
      { kind: "type", tool: "wtype" },
      { kind: "type", tool: "ydotool" },
    ]);
    expect(linuxPastePlan(false, false)).toEqual([
      { kind: "type", tool: "xdotool" },
      { kind: "type", tool: "ydotool" },
    ]);
  });

  it("never plans wtype for Ctrl+V or xdotool under Wayland", () => {
    // wtype has no key command; xdotool under Wayland only reaches
    // XWayland and would report success while typing into nothing.
    for (const step of linuxPastePlan(true, true)) {
      expect(step.tool).not.toBe("xdotool");
      if (step.kind === "ctrl_v") expect(step.tool).not.toBe("wtype");
    }
  });
});

describe("linuxPasteStepArgs", () => {
  it("synthesizes Ctrl+V as ydotool key codes and xdotool key", () => {
    expect(linuxPasteStepArgs({ kind: "ctrl_v", tool: "ydotool" }, "ignored")).toEqual([
      "key",
      "--delay",
      "0",
      "--key-delay",
      "2",
      "29:1",
      "47:1",
      "47:0",
      "29:0",
    ]);
    expect(linuxPasteStepArgs({ kind: "ctrl_v", tool: "xdotool" }, "ignored")).toEqual([
      "key",
      "--clearmodifiers",
      "ctrl+v",
    ]);
  });

  it("types the text after an argument separator", () => {
    expect(linuxPasteStepArgs({ kind: "type", tool: "wtype" }, "hello --kbd")).toEqual([
      "--",
      "hello --kbd",
    ]);
    expect(linuxPasteStepArgs({ kind: "type", tool: "ydotool" }, "hi")).toEqual([
      "type",
      "--delay",
      "0",
      "--",
      "hi",
    ]);
    expect(linuxPasteStepArgs({ kind: "type", tool: "xdotool" }, "hi")).toEqual([
      "type",
      "--clearmodifiers",
      "--",
      "hi",
    ]);
  });
});

describe("clipboardHoldsText", () => {
  const noSleep = async () => {};

  it("matches an immediate read-back without sleeping", async () => {
    const sleep = vi.fn(noSleep);
    await expect(clipboardHoldsText("abc", () => "abc", sleep)).resolves.toBe(true);
    expect(sleep).not.toHaveBeenCalled();
  });

  it("retries until the propagated write becomes visible", async () => {
    const sleep = vi.fn(noSleep);
    let reads = 0;
    const readText = () => (reads++ < 2 ? "stale" : "abc");
    await expect(clipboardHoldsText("abc", readText, sleep)).resolves.toBe(true);
    expect(reads).toBe(3);
    expect(sleep).toHaveBeenCalledTimes(2); // waits before retries only
  });

  it("gives up after the bounded retry window", async () => {
    const sleep = vi.fn(noSleep);
    await expect(clipboardHoldsText("abc", () => "stale", sleep)).resolves.toBe(false);
    // One read per configured wait.
    expect(sleep).toHaveBeenCalledTimes(3);
  });

  it("treats a throwing read as unverified, not as a failure", async () => {
    await expect(clipboardHoldsText("abc", () => {
      throw new Error("no clipboard");
    }, noSleep)).resolves.toBe(false);
  });
});

// ── macOS/Windows arm: sidecar native paste (canario-ubb) ──────────────

describe("autoPasteText (non-Linux)", () => {
  const realPlatform = process.platform;

  beforeEach(() => {
    vi.clearAllMocks();
    // A non-Linux platform takes the sidecar arm.
    Object.defineProperty(process, "platform", { value: "darwin", configurable: true });
  });
  afterEach(() => {
    Object.defineProperty(process, "platform", { value: realPlatform, configurable: true });
  });

  it("writes the clipboard, verifies it, then sends paste_text to the sidecar", async () => {
    readText.mockResolvedValue("hello");
    sendCommandMock.mockResolvedValue({ ok: true, data: { pasted: true } });

    await expect(autoPasteText("hello")).resolves.toBe(true);
    expect(writeText).toHaveBeenCalledWith("hello");
    expect(sendCommandMock).toHaveBeenCalledTimes(1);
    const cmd = sendCommandMock.mock.calls[0][0] as Record<string, unknown>;
    expect(cmd.cmd).toBe("paste_text");
    expect(cmd.text).toBe("hello");
  });

  it("never sends the chord when the clipboard read-back cannot verify", async () => {
    readText.mockResolvedValue("stale-previous-content");
    await expect(autoPasteText("hello")).resolves.toBe(false);
    expect(sendCommandMock).not.toHaveBeenCalled();
  });

  it("reports false (not an error) when the sidecar backends fail to deliver", async () => {
    readText.mockResolvedValue("hello");
    sendCommandMock.mockResolvedValue({ ok: true, data: { pasted: false } });
    await expect(autoPasteText("hello")).resolves.toBe(false);
  });

  it("survives a sidecar round-trip error", async () => {
    readText.mockResolvedValue("hello");
    sendCommandMock.mockRejectedValue(new Error("sidecar gone"));
    await expect(autoPasteText("hello")).resolves.toBe(false);
  });
});

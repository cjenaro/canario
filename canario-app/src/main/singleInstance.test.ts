// Tests for the single-instance guard (bead canario-tem): the duplicate
// instance must be detected before any sidecar/tray/window work, and the
// primary instance must restore/focus its existing window on second-instance.
import { describe, it, expect, vi, beforeEach, afterEach, type Mock } from "vitest";

vi.mock("electron", () => ({
  app: {
    requestSingleInstanceLock: vi.fn(),
    on: vi.fn(),
    quit: vi.fn(),
    dock: { show: vi.fn() },
  },
}));

import { app } from "electron";
import {
  acquireSingleInstanceLock,
  focusExistingWindow,
  type FocusableWindow,
} from "./singleInstance";

type MockWindow = {
  isMinimized: () => boolean;
  restore: ReturnType<typeof vi.fn>;
  show: ReturnType<typeof vi.fn>;
  focus: ReturnType<typeof vi.fn>;
};

function fakeWindow(minimized = false): MockWindow {
  return {
    isMinimized: () => minimized,
    restore: vi.fn(),
    show: vi.fn(),
    focus: vi.fn(),
  };
}

const lockMock = vi.mocked(app.requestSingleInstanceLock);
// Electron's `app.on` is heavily overloaded (one tuple per event name), so
// type the mock loosely and assert on the raw call args.
const onMock = app.on as unknown as Mock;

let originalPlatform: string;

function stubPlatform(platform: string) {
  Object.defineProperty(process, "platform", {
    value: platform,
    configurable: true,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  if (originalPlatform !== undefined) {
    stubPlatform(originalPlatform);
    originalPlatform = undefined as unknown as string;
  }
});

describe("acquireSingleInstanceLock", () => {
  it("reports a duplicate when the lock is denied, without wiring second-instance", () => {
    lockMock.mockReturnValue(false);

    const gotLock = acquireSingleInstanceLock(() => null);

    expect(gotLock).toBe(false);
    expect(lockMock).toHaveBeenCalledOnce();
    // A duplicate quits immediately — it must not register any lifecycle
    // handler that could let startup (sidecar/tray) proceed.
    expect(onMock).not.toHaveBeenCalled();
  });

  it("registers a second-instance handler for the primary instance", () => {
    lockMock.mockReturnValue(true);

    const gotLock = acquireSingleInstanceLock(() => null);

    expect(gotLock).toBe(true);
    expect(onMock.mock.calls.some(([event]) => event === "second-instance")).toBe(true);
  });

  it("focuses the existing window when the second-instance handler fires", () => {
    lockMock.mockReturnValue(true);
    const win = fakeWindow(true);
    acquireSingleInstanceLock(() => win as unknown as FocusableWindow);

    const handler = onMock.mock.calls.find(([event]) => event === "second-instance")![1] as () => void;
    handler();

    expect(win.restore).toHaveBeenCalledOnce();
    expect(win.show).toHaveBeenCalledOnce();
    expect(win.focus).toHaveBeenCalledOnce();
  });

  it("is a no-op when second-instance fires before the window exists (startup timing)", () => {
    lockMock.mockReturnValue(true);
    acquireSingleInstanceLock(() => null);

    const handler = onMock.mock.calls.find(([event]) => event === "second-instance")![1] as () => void;

    expect(() => handler()).not.toThrow();
    // No window to surface — not even the Dock icon should be touched.
    expect(app.dock?.show).not.toHaveBeenCalled();
  });
});

describe("focusExistingWindow", () => {
  it("restores a minimized window, then shows and focuses it", () => {
    const win = fakeWindow(true);

    focusExistingWindow(win as unknown as FocusableWindow);

    expect(win.restore).toHaveBeenCalledOnce();
    expect(win.show).toHaveBeenCalledOnce();
    expect(win.focus).toHaveBeenCalledOnce();
  });

  it("does not restore a window that is not minimized", () => {
    const win = fakeWindow(false);

    focusExistingWindow(win as unknown as FocusableWindow);

    expect(win.restore).not.toHaveBeenCalled();
    expect(win.show).toHaveBeenCalledOnce();
    expect(win.focus).toHaveBeenCalledOnce();
  });

  it("shows the Dock icon on macOS", () => {
    originalPlatform = process.platform;
    stubPlatform("darwin");
    const win = fakeWindow();

    focusExistingWindow(win as unknown as FocusableWindow);

    expect(app.dock?.show).toHaveBeenCalledOnce();
  });

  it("leaves the Dock alone on Linux", () => {
    originalPlatform = process.platform;
    stubPlatform("linux");
    const win = fakeWindow();

    focusExistingWindow(win as unknown as FocusableWindow);

    expect(app.dock?.show).not.toHaveBeenCalled();
  });
});

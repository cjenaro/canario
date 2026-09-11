// Tests for the hotkey health primitive: payload parsing and the
// gating rule that decides when the permission guidance is shown.
import { describe, it, expect } from "vitest";
import {
  parseHotkeyStatus,
  shouldShowHotkeyPermissionNotice,
  type HotkeyStatusInfo,
} from "./hotkeyStatus";

const denied: HotkeyStatusInfo = {
  backend: "socket-fallback",
  permission_denied: true,
  fix_command: "sudo usermod -aG input $USER",
  detail: "No keyboard devices readable in /dev/input",
};

describe("parseHotkeyStatus", () => {
  it("parses a full payload", () => {
    expect(
      parseHotkeyStatus({
        backend: "evdev",
        permission_denied: false,
        fix_command: null,
        detail: null,
      }),
    ).toEqual({
      backend: "evdev",
      permission_denied: false,
      fix_command: null,
      detail: null,
    });
  });

  it("rejects non-objects", () => {
    expect(parseHotkeyStatus(null)).toBeNull();
    expect(parseHotkeyStatus("evdev")).toBeNull();
    expect(parseHotkeyStatus(42)).toBeNull();
  });

  it("rejects payloads missing required typed fields", () => {
    expect(parseHotkeyStatus({})).toBeNull();
    expect(parseHotkeyStatus({ backend: "evdev" })).toBeNull();
    expect(parseHotkeyStatus({ backend: 7, permission_denied: false })).toBeNull();
    expect(parseHotkeyStatus({ backend: "evdev", permission_denied: "yes" })).toBeNull();
  });

  it("tolerates absent optional fields", () => {
    expect(parseHotkeyStatus({ backend: "x11", permission_denied: false })).toEqual({
      backend: "x11",
      permission_denied: false,
      fix_command: null,
      detail: null,
    });
  });
});

describe("shouldShowHotkeyPermissionNotice", () => {
  it("shows for a permissions failure on Linux", () => {
    expect(shouldShowHotkeyPermissionNotice(denied, true)).toBe(true);
  });

  it("never shows on non-Linux platforms (Electron shortcuts handle the hotkey)", () => {
    expect(shouldShowHotkeyPermissionNotice(denied, false)).toBe(false);
  });

  it("does not show before the hotkey listener has started", () => {
    expect(
      shouldShowHotkeyPermissionNotice(
        { backend: "not-started", permission_denied: false, fix_command: null, detail: null },
        true,
      ),
    ).toBe(false);
  });

  it("does not show for healthy backends", () => {
    expect(
      shouldShowHotkeyPermissionNotice(
        { backend: "evdev", permission_denied: false, fix_command: null, detail: null },
        true,
      ),
    ).toBe(false);
    expect(
      shouldShowHotkeyPermissionNotice(
        { backend: "x11", permission_denied: false, fix_command: null, detail: null },
        true,
      ),
    ).toBe(false);
  });

  it("does not show for a socket fallback with a non-permission cause", () => {
    expect(
      shouldShowHotkeyPermissionNotice(
        { backend: "socket-fallback", permission_denied: false, fix_command: null, detail: "no devices" },
        true,
      ),
    ).toBe(false);
  });

  it("stays hidden for null/undefined status (query failed or never ran)", () => {
    expect(shouldShowHotkeyPermissionNotice(null, true)).toBe(false);
    expect(shouldShowHotkeyPermissionNotice(undefined, true)).toBe(false);
  });

  it("defensively requires a non-empty fix command alongside the denial", () => {
    expect(
      shouldShowHotkeyPermissionNotice(
        { ...denied, fix_command: null },
        true,
      ),
    ).toBe(false);
    expect(shouldShowHotkeyPermissionNotice({ ...denied, fix_command: "" }, true)).toBe(false);
  });
});

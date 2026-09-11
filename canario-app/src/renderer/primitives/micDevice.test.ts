// Tests for the microphone device-picker helpers (canario-1hq.2) —
// list_audio_devices response parsing, the AppConfig input_device
// key + update payload, and the dropdown options ("System default"
// first, missing-but-persisted devices kept selectable).
import { describe, it, expect } from "vitest";
import {
  inputDeviceConfigPayload,
  inputDeviceFromConfig,
  micDevicesFromResponse,
  micDropdownOptions,
  SYSTEM_DEFAULT_INPUT_DEVICE,
  SYSTEM_DEFAULT_OPTION_LABEL,
} from "./micDevice";

describe("micDevicesFromResponse", () => {
  it("reads an ok array of {name} entries", () => {
    expect(
      micDevicesFromResponse({ ok: true, data: [{ name: "Mic A" }, { name: "Mic B" }] }),
    ).toEqual([{ name: "Mic A" }, { name: "Mic B" }]);
  });

  it("drops entries without a usable name", () => {
    expect(
      micDevicesFromResponse({
        ok: true,
        data: [{ name: "Mic A" }, {}, { name: "" }, { name: 7 }, null],
      }),
    ).toEqual([{ name: "Mic A" }]);
  });

  it("degrades to an empty list for non-ok or malformed responses", () => {
    expect(micDevicesFromResponse({ ok: false, error: "boom" })).toEqual([]);
    expect(micDevicesFromResponse({ ok: true, data: "not-an-array" })).toEqual([]);
    expect(micDevicesFromResponse({ ok: true })).toEqual([]);
    expect(micDevicesFromResponse(null)).toEqual([]);
  });
});

describe("inputDeviceFromConfig / inputDeviceConfigPayload", () => {
  it("reads AppConfig.input_device, defaulting to the system default", () => {
    expect(inputDeviceFromConfig({ input_device: "Yeti SB" })).toBe("Yeti SB");
    expect(inputDeviceFromConfig({})).toBe(SYSTEM_DEFAULT_INPUT_DEVICE);
    expect(inputDeviceFromConfig(null)).toBe(SYSTEM_DEFAULT_INPUT_DEVICE);
    expect(inputDeviceFromConfig({ input_device: 42 })).toBe(SYSTEM_DEFAULT_INPUT_DEVICE);
  });

  it("sends input_device as its own top-level key", () => {
    expect(inputDeviceConfigPayload("Yeti SB")).toEqual({ input_device: "Yeti SB" });
    expect(inputDeviceConfigPayload("")).toEqual({ input_device: "" });
    // Exactly one key travels — update_config merges top-level keys
    // wholesale, so nothing else can be clobbered by a device switch.
    expect(Object.keys(inputDeviceConfigPayload("Yeti SB"))).toEqual(["input_device"]);
  });
});

describe("micDropdownOptions", () => {
  it("offers System default first, then the enumerated devices in order", () => {
    expect(micDropdownOptions([{ name: "Mic A" }, { name: "Mic B" }])).toEqual([
      { value: SYSTEM_DEFAULT_INPUT_DEVICE, label: SYSTEM_DEFAULT_OPTION_LABEL },
      { value: "Mic A", label: "Mic A" },
      { value: "Mic B", label: "Mic B" },
    ]);
  });

  it("offers System default alone when no devices enumerate", () => {
    expect(micDropdownOptions([])).toEqual([
      { value: SYSTEM_DEFAULT_INPUT_DEVICE, label: SYSTEM_DEFAULT_OPTION_LABEL },
    ]);
  });

  it("keeps a persisted-but-missing device selectable, marked not connected", () => {
    const options = micDropdownOptions([{ name: "Mic A" }], "Unplugged");
    expect(options[0]).toEqual({ value: "", label: SYSTEM_DEFAULT_OPTION_LABEL });
    expect(options).toContainEqual({ value: "Unplugged", label: "Unplugged (not connected)" });
  });

  it("does not duplicate a selected device that is still enumerated", () => {
    expect(micDropdownOptions([{ name: "Mic A" }], "Mic A")).toHaveLength(2);
    // The system-default sentinel never gains a "(not connected)" row.
    expect(micDropdownOptions([{ name: "Mic A" }], "")).toHaveLength(2);
    // No selection provided behaves like the sentinel.
    expect(micDropdownOptions([{ name: "Mic A" }], undefined)).toHaveLength(2);
  });
});

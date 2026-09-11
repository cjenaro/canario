// Microphone device selection — pure logic for the Settings
// "Microphone" section (canario-1hq.2): parsing the sidecar's
// list_audio_devices response, reading/writing the AppConfig
// input_device key, and building the dropdown's options ("System
// default" first).
//
// No DOM, no IPC — MicSection/AppPage own those so this module stays
// node-testable (mirrors primitives/animations.ts).

/** Wire shape of a list_audio_devices entry (mirrors core's MicDevice). */
export interface MicDevice {
  name: string;
}

/** The dropdown/config sentinel for "no preference — use the system default". */
export const SYSTEM_DEFAULT_INPUT_DEVICE = "";

/** Default label of the dropdown's first option (English — i18n callers
 *  override via micDropdownOptions' labels parameter; the default keeps
 *  this module's node tests free of any i18n dependency). */
export const SYSTEM_DEFAULT_OPTION_LABEL = "System default";

/** Dropdown label strings — supplied by the caller (the i18n catalog in
 *  MicSection); the English defaults below keep this module standalone. */
export interface MicOptionLabels {
  systemDefault: string;
  notConnected: (name: string) => string;
}

export const DEFAULT_MIC_OPTION_LABELS: MicOptionLabels = {
  systemDefault: SYSTEM_DEFAULT_OPTION_LABEL,
  notConnected: (name) => `${name} (not connected)`,
};

/**
 * Extract the input-device names from a list_audio_devices response.
 * Lenient: a non-ok response, a non-array `data`, or entries without
 * a usable name degrade to an empty list — the picker then offers
 * only "System default" instead of throwing (the sidecar already
 * never errors this command; this covers a stale/odd peer too).
 */
export function micDevicesFromResponse(response: unknown): MicDevice[] {
  const res = (response ?? {}) as Record<string, unknown>;
  if (res.ok !== true || !Array.isArray(res.data)) return [];
  return res.data
    .map((entry) => (entry ?? {}) as Record<string, unknown>)
    .filter((entry): entry is { name: string } =>
      typeof entry.name === "string" && entry.name.length > 0)
    .map((entry) => ({ name: entry.name }));
}

/**
 * Read AppConfig.input_device — "" (or anything not a string) means
 * the system default (old configs, pre-picker).
 */
export function inputDeviceFromConfig(config: unknown): string {
  const cfg = (config ?? {}) as Record<string, unknown>;
  return typeof cfg.input_device === "string" ? cfg.input_device : "";
}

/**
 * The update_config payload for a device choice: input_device is a
 * plain top-level AppConfig key sent on its own — update_config
 * merges top-level keys wholesale, so nothing else can be clobbered
 * and the sidecar pushes the new value into the warm-mic preference.
 */
export function inputDeviceConfigPayload(device: string): { input_device: string } {
  return { input_device: device };
}

/** One dropdown option. */
export interface MicDeviceOption {
  /** The value persisted to AppConfig.input_device ("" = default). */
  value: string;
  /** The label shown in the dropdown. */
  label: string;
}

/**
 * The dropdown's options: "System default" first, then the enumerated
 * devices in the host's order. A persisted device missing from the
 * enumeration (unplugged since the last settings visit) stays
 * selectable, marked "(not connected)" — the dropdown never silently
 * shows the wrong row, and the core falls back to the system default
 * while it stays missing.
 */
export function micDropdownOptions(
  devices: MicDevice[],
  selected?: string,
  labels: MicOptionLabels = DEFAULT_MIC_OPTION_LABELS,
): MicDeviceOption[] {
  const options: MicDeviceOption[] = [
    { value: SYSTEM_DEFAULT_INPUT_DEVICE, label: labels.systemDefault },
    ...devices.map((d) => ({ value: d.name, label: d.name })),
  ];
  if (
    selected !== undefined &&
    selected !== SYSTEM_DEFAULT_INPUT_DEVICE &&
    !devices.some((d) => d.name === selected)
  ) {
    options.push({ value: selected, label: labels.notConnected(selected) });
  }
  return options;
}

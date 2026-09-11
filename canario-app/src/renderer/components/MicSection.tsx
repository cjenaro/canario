// Microphone section content — input device picker for dictation
// (canario-1hq.2).
//
// Pure presentation: AppPage owns the state and the persistence
// (input_device as its own top-level AppConfig key via update_config);
// response parsing and dropdown construction live in
// primitives/micDevice.ts. Re-enumerates when the section opens so
// hotplug (USB plug, Bluetooth connect) is reflected.
import { For, onMount } from "solid-js";
import { micDropdownOptions, type MicDevice } from "../primitives/micDevice";

interface Props {
  /** Enumerated devices (list_audio_devices; empty = default only). */
  devices: MicDevice[];
  /** The persisted input_device ("" = system default). */
  selected: string;
  /** Persist a new choice — receives the raw device name ("" = default). */
  onDeviceChange: (device: string) => void;
  /** Re-enumerate the device list (called when the section opens). */
  onRefresh: () => void;
}

export function MicSection(props: Props) {
  // Re-enumerate on section open: the device list changes with
  // hotplug between settings visits.
  onMount(() => props.onRefresh());

  return (
    <div class="flex flex-col gap-2">
      <div class="flex items-center justify-between">
        <div>
          <p class="text-sm font-medium">Dictation microphone</p>
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            Which input device Canario records from
          </p>
        </div>
        <select
          class="px-3 py-1.5 rounded-lg border text-sm max-w-56"
          style={{
            "background-color": "var(--bg)",
            "border-color": "var(--border)",
            color: "var(--text-primary)",
            cursor: "pointer",
          }}
          value={props.selected}
          onChange={(e) => props.onDeviceChange(e.currentTarget.value)}
        >
          <For each={micDropdownOptions(props.devices, props.selected)}>
            {(option) => <option value={option.value}>{option.label}</option>}
          </For>
        </select>
      </div>
      <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
        Switching releases the warm microphone stream and reopens it on the new device at the next
        dictation.
      </p>
    </div>
  );
}

// Persistent guidance for the Linux evdev "user not in the input group"
// failure. Rendered inside the Hotkey settings section — deliberately
// NOT a toast: the condition survives restarts until the user acts on
// it, so the guidance must too.
import { showToast } from "./Toast";
import type { HotkeyStatusInfo } from "../primitives/hotkeyStatus";

/** Fallback shown if the sidecar ever omits fix_command unexpectedly. */
const DEFAULT_FIX_COMMAND = "sudo usermod -aG input $USER";

export function HotkeyPermissionNotice(props: { status: HotkeyStatusInfo }) {
  const command = () => props.status.fix_command || DEFAULT_FIX_COMMAND;

  async function copyCommand() {
    try {
      await navigator.clipboard.writeText(command());
      showToast("Command copied to clipboard", "success", 3000);
    } catch {
      showToast("Could not copy — select the command text and copy it manually", "error", 6000);
    }
  }

  return (
    <div
      class="rounded-xl border p-4 flex items-start gap-3"
      style={{
        "background-color": "rgba(251, 191, 36, 0.06)",
        "border-color": "rgba(251, 191, 36, 0.3)",
      }}
    >
      <span class="text-lg leading-none mt-0.5">⌨️</span>
      <div class="flex-1">
        <p class="text-sm font-medium" style={{ color: "var(--warning)" }}>
          Hotkey can’t read your keyboard yet
        </p>
        <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
          On Linux, Canario listens for the global hotkey through /dev/input, and your user
          isn’t in the “input” group — so the hotkey stays silent. Recording still works from
          the button above, the tray, or an external trigger such as{" "}
          <code>canario-cli --toggle-external</code>.
        </p>
        <div class="mt-2 flex items-center gap-2">
          <code
            class="flex-1 text-xs px-2 py-1.5 rounded-md overflow-x-auto whitespace-nowrap"
            style={{
              "background-color": "var(--surface)",
              border: "1px solid var(--border)",
              color: "var(--warning)",
            }}
          >
            {command()}
          </code>
          <button
            class="text-xs px-2 py-1.5 rounded-md hover:opacity-80 transition-opacity whitespace-nowrap"
            style={{
              color: "var(--text-secondary)",
              cursor: "pointer",
              "background-color": "var(--surface)",
              border: "1px solid var(--border)",
            }}
            onClick={() => void copyCommand()}
            title="Copy the command to the clipboard"
          >
            📋 Copy
          </button>
        </div>
        <ol
          class="text-xs mt-2 space-y-0.5 list-decimal list-inside"
          style={{ color: "var(--text-secondary)" }}
        >
          <li>Copy the command and run it in a terminal.</li>
          <li>Log out and back in — group membership only applies to new sessions.</li>
          <li>Start Canario again.</li>
        </ol>
      </div>
    </div>
  );
}

// Hotkey capture widget — captures key combos for hotkey configuration
import { createSignal, Show, onCleanup, onMount } from "solid-js";

interface Props {
  /** Current hotkey keys (e.g., ["Super", "Alt", "Space"]) */
  value: string[];
  /** Called with new key combo when captured */
  onChange: (keys: string[]) => void;
}

// Map DOM key names to user-friendly display names
const KEY_DISPLAY: Record<string, string> = {
  Meta: "Super",
  OS: "Super",
  Control: "Ctrl",
  Alt: "Alt",
  Shift: "Shift",
  " ": "Space",
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Escape: "Esc",
  Backspace: "⌫",
  Delete: "Del",
  Return: "Enter",
};

// Map DOM key values to config key names
function toKeyName(key: string): string {
  return KEY_DISPLAY[key] || key.charAt(0).toUpperCase() + key.slice(1);
}

// Electron accelerator format
function toAccelerator(keys: string[]): string {
  return keys
    .map((k) => {
      const lower = k.toLowerCase();
      if (lower === "super") return "Super";
      if (lower === "ctrl") return "Ctrl";
      if (lower === "alt") return "Alt";
      if (lower === "shift") return "Shift";
      if (lower === "space") return "Space";
      return k;
    })
    .join("+");
}

export function HotkeyCapture(props: Props) {
  const [capturing, setCapturing] = createSignal(false);
  const [currentKeys, setCurrentKeys] = createSignal<string[]>([]);

  // Display the key combo
  const displayKeys = () => {
    const keys = capturing() ? currentKeys() : props.value;
    if (keys.length === 0) return "Not set";
    return keys.join(" + ");
  };

  function handleKeyDown(e: KeyboardEvent) {
    if (!capturing()) return;
    e.preventDefault();
    e.stopPropagation();

    // Ignore lone modifier presses — wait for a non-modifier key
    const modifierKeys = new Set(["Control", "Alt", "Shift", "Meta"]);
    if (modifierKeys.has(e.key) && currentKeys().length === 0) {
      // Build modifier combo as they press modifiers
      const mods: string[] = [];
      if (e.ctrlKey) mods.push("Ctrl");
      if (e.altKey) mods.push("Alt");
      if (e.shiftKey) mods.push("Shift");
      if (e.metaKey) mods.push("Super");
      setCurrentKeys(mods);
      return;
    }

    // Build full combo from modifier state + the final key
    const keys: string[] = [];
    if (e.ctrlKey) keys.push("Ctrl");
    if (e.altKey) keys.push("Alt");
    if (e.shiftKey) keys.push("Shift");
    if (e.metaKey) keys.push("Super");

    // Add the non-modifier key (if it's not just a modifier by itself)
    if (!modifierKeys.has(e.key)) {
      keys.push(toKeyName(e.key));
    }

    if (keys.length > 0 && !modifierKeys.has(e.key)) {
      // Escape to cancel
      if (e.key === "Escape") {
        setCapturing(false);
        setCurrentKeys([]);
        return;
      }

      setCurrentKeys(keys);
      // Accept the combo on keydown
      setCapturing(false);
      props.onChange(keys);
    }
  }

  // Capture key events globally while in capture mode
  onMount(() => {
    window.addEventListener("keydown", handleKeyDown, true);
    onCleanup(() => window.removeEventListener("keydown", handleKeyDown, true));
  });

  return (
    <div class="flex items-center gap-2">
      <button
        class="flex-1 px-3 py-2 rounded-lg border text-sm text-center transition-colors"
        style={{
          "background-color": capturing() ? "var(--accent)" : "var(--bg)",
          "border-color": capturing() ? "var(--accent)" : "var(--border)",
          color: capturing() ? "white" : "var(--text-primary)",
          cursor: "pointer",
          "min-height": "38px",
        }}
        onClick={() => {
          if (capturing()) {
            setCapturing(false);
            setCurrentKeys([]);
          } else {
            setCapturing(true);
            setCurrentKeys([]);
          }
        }}
      >
        <Show when={capturing()} fallback={displayKeys()}>
          Press key combination… <span style={{ opacity: 0.6 }}>(Esc to cancel)</span>
        </Show>
      </button>
      <Show when={!capturing()}>
        <button
          class="px-3 py-2 rounded-lg border text-sm transition-colors"
          style={{
            "background-color": "var(--surface-hover)",
            "border-color": "var(--border)",
            color: "var(--text-primary)",
            cursor: "pointer",
          }}
          onClick={() => {
            setCapturing(true);
            setCurrentKeys([]);
          }}
        >
          Change
        </button>
      </Show>
    </div>
  );
}

export { toAccelerator };

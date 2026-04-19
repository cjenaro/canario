// Toast notification system — transient messages for the settings window
// Uses Solid signals at module scope for global access

import { createSignal, For, Show } from "solid-js";

export interface Toast {
  id: number;
  message: string;
  type: "error" | "success" | "info" | "warning";
}

let nextId = 0;
const [toasts, setToasts] = createSignal<Toast[]>([]);
export { toasts };

/** Show a toast notification that auto-dismisses */
export function showToast(message: string, type: Toast["type"] = "info", duration = 4000) {
  const id = ++nextId;
  setToasts((prev) => [...prev, { id, message, type }]);
  setTimeout(() => {
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, duration);
}

/** Dismiss a specific toast */
export function dismissToast(id: number) {
  setToasts((prev) => prev.filter((t) => t.id !== id));
}

// ── Toast container component ──────────────────────────────────────

const TOAST_STYLES: Record<Toast["type"], { bg: string; border: string; icon: string }> = {
  error: { bg: "rgba(239, 68, 68, 0.1)", border: "rgba(239, 68, 68, 0.3)", icon: "⚠" },
  success: { bg: "rgba(74, 222, 128, 0.1)", border: "rgba(74, 222, 128, 0.3)", icon: "✓" },
  warning: { bg: "rgba(251, 191, 36, 0.1)", border: "rgba(251, 191, 36, 0.3)", icon: "⚡" },
  info: { bg: "rgba(136, 136, 168, 0.1)", border: "rgba(136, 136, 168, 0.3)", icon: "ℹ" },
};

export function ToastContainer() {
  return (
    <Show when={toasts().length > 0}>
      <div class="toast-container">
        <For each={toasts()}>
          {(toast) => {
            const style = TOAST_STYLES[toast.type];
            return (
              <div class="toast-item animate-toast-in" style={{ "background-color": style.bg, "border-color": style.border }}>
                <span class="toast-icon">{style.icon}</span>
                <span class="toast-message">{toast.message}</span>
                <button class="toast-dismiss" onClick={() => dismissToast(toast.id)}>
                  ✕
                </button>
              </div>
            );
          }}
        </For>
      </div>
    </Show>
  );
}

// Indicator section content — the on-screen indicator style while
// dictating (canario-aud.2): the full overlay (default), a minimal
// recording dot, or no indicator with the tray icon carrying the
// signal. Pure presentation: AppPage owns the state and the
// persistence.
import { For, Show } from "solid-js";
import { t, type MessageKey } from "../i18n";
import {
  OVERLAY_PRESENCE_MODES,
  type OverlayPresence,
} from "../primitives/overlayPresence";

const MODE_META_KEYS: Record<OverlayPresence, { name: MessageKey; desc: MessageKey }> = {
  full: {
    name: "indicator.full.name",
    desc: "indicator.full.desc",
  },
  dot: {
    name: "indicator.dot.name",
    desc: "indicator.dot.desc",
  },
  tray: {
    name: "indicator.tray.name",
    desc: "indicator.tray.desc",
  },
};

interface Props {
  mode: OverlayPresence;
  onChange: (mode: OverlayPresence) => void;
}

export function IndicatorSection(props: Props) {
  return (
    <div class="flex flex-col gap-2">
      <For each={OVERLAY_PRESENCE_MODES}>
        {(m) => (
          <button
            class="flex items-center justify-between gap-3 p-3 rounded-lg border transition-colors cursor-pointer"
            style={{
              "background-color": props.mode === m ? "var(--surface-hover)" : "transparent",
              "border-color": props.mode === m ? "var(--accent)" : "var(--border)",
            }}
            aria-pressed={props.mode === m}
            onClick={() => props.onChange(m)}
          >
            <div class="text-left">
              <p class="text-sm font-medium">{t(MODE_META_KEYS[m].name)}</p>
              <p class="text-xs mt-0.5" style={{ color: "var(--text-secondary)" }}>
                {t(MODE_META_KEYS[m].desc)}
              </p>
            </div>
            <div
              class="w-4 h-4 rounded-full border-2 flex items-center justify-center shrink-0"
              style={{ "border-color": props.mode === m ? "var(--accent)" : "var(--border)" }}
              aria-hidden="true"
            >
              <Show when={props.mode === m}>
                <div class="w-2 h-2 rounded-full" style={{ "background-color": "var(--accent)" }} />
              </Show>
            </div>
          </button>
        )}
      </For>
      <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
        {t("indicator.note")}
      </p>
    </div>
  );
}

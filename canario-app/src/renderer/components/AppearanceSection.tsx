// Appearance section content — theme mode (dark/light/system) + accent
// color (preset swatches + custom hex with validation).
// Pure presentation: AppPage owns the state and the persistence.
import { createEffect, createSignal, For, Show } from "solid-js";
import { t, type MessageKey } from "../i18n";
import {
  ACCENT_PRESETS,
  isHexColor,
  normalizeHexColor,
  THEME_MODES,
  type AccentColor,
  type ThemeMode,
} from "../primitives/appearance";

const MODE_LABEL_KEYS: Record<ThemeMode, MessageKey> = {
  dark: "appearance.mode.dark",
  light: "appearance.mode.light",
  system: "appearance.mode.system",
};

// Accent preset display names, keyed by preset id (the hex/value table
// stays in primitives/appearance.ts — only the label is localized).
const PRESET_NAME_KEYS = {
  canary: "appearance.accent.preset.canary",
  ocean: "appearance.accent.preset.ocean",
  violet: "appearance.accent.preset.violet",
  emerald: "appearance.accent.preset.emerald",
  amber: "appearance.accent.preset.amber",
  rose: "appearance.accent.preset.rose",
} as const satisfies Record<string, MessageKey>;

interface Props {
  mode: ThemeMode;
  accent: AccentColor;
  onModeChange: (mode: ThemeMode) => void;
  onAccentChange: (accent: AccentColor) => void;
}

export function AppearanceSection(props: Props) {
  // Custom hex entry. Local only — it commits (onAccentChange) on
  // Apply, Enter, or blur with a valid value, so the config isn't
  // written on every keystroke.
  const [customHex, setCustomHex] = createSignal("");
  const [touched, setTouched] = createSignal(false);

  // Keep the input in sync with the active accent: pre-filled while a
  // custom hex is active, cleared when a preset (or the default) wins.
  createEffect(() => {
    const current = props.accent;
    setCustomText(current !== null && !ACCENT_PRESETS.some((p) => p.hex === current) ? current : "");
  });

  function setCustomText(text: string) {
    setCustomHex(text);
    setTouched(false);
  }

  const trimmed = () => customHex().trim();
  const customValid = () => trimmed() !== "" && isHexColor(trimmed());
  const normalizedCustom = () => normalizeHexColor(trimmed());
  const showHexError = () => touched() && trimmed() !== "" && !customValid();

  function commitCustom() {
    const hex = normalizedCustom();
    if (!hex) return;
    props.onAccentChange(hex);
  }

  const swatchStyle = (active: boolean, background: string) => ({
    "background-color": background,
    "border-color": active ? "var(--text-primary)" : "var(--border)",
  }) as const;

  return (
    <div class="flex flex-col">
      {/* Theme mode */}
      <div class="flex gap-2">
        <For each={THEME_MODES}>
          {(m) => (
            <button
              class="flex-1 px-3 py-2 rounded-lg border text-sm font-medium transition-colors"
              style={{
                "background-color": props.mode === m ? "var(--surface-hover)" : "transparent",
                "border-color": props.mode === m ? "var(--accent)" : "var(--border)",
                color: "var(--text-primary)",
                cursor: "pointer",
              }}
              aria-pressed={props.mode === m}
              onClick={() => props.onModeChange(m)}
            >
              {t(MODE_LABEL_KEYS[m])}
            </button>
          )}
        </For>
      </div>

      {/* Accent color */}
      <div class="mt-4">
        <p class="text-sm font-medium">{t("appearance.accent.title")}</p>
        <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
          {t("appearance.accent.desc")}
        </p>

        <div class="flex items-center gap-2 mt-2.5 flex-wrap">
          {/* Default (per-theme) accent */}
          <button
            class="w-7 h-7 rounded-full border-2 flex items-center justify-center transition-transform hover:scale-105"
            style={swatchStyle(props.accent === null, "var(--accent-default)")}
            title={t("appearance.accent.defaultTitle")}
            aria-label={t("appearance.accent.defaultLabel")}
            aria-pressed={props.accent === null}
            onClick={() => props.onAccentChange(null)}
          >
            <Show when={props.accent === null}>
              <span class="text-xs font-bold" style={{ color: "white" }}>✓</span>
            </Show>
          </button>

          <For each={ACCENT_PRESETS}>
            {(preset) => (
              <button
                class="w-7 h-7 rounded-full border-2 flex items-center justify-center transition-transform hover:scale-105"
                style={swatchStyle(props.accent === preset.hex, preset.hex)}
                title={t(PRESET_NAME_KEYS[preset.id as keyof typeof PRESET_NAME_KEYS])}
                aria-label={t("appearance.accent.presetAria", {
                  name: t(PRESET_NAME_KEYS[preset.id as keyof typeof PRESET_NAME_KEYS]),
                })}
                aria-pressed={props.accent === preset.hex}
                onClick={() => props.onAccentChange(preset.hex)}
              >
                <Show when={props.accent === preset.hex}>
                  <span class="text-xs font-bold" style={{ color: "white" }}>✓</span>
                </Show>
              </button>
            )}
          </For>
        </div>

        {/* Custom hex */}
        <div class="flex items-center gap-2 mt-3">
          <span class="text-xs w-14 shrink-0" style={{ color: "var(--text-secondary)" }}>
            {t("appearance.accent.custom")}
          </span>
          {/* Live preview chip of the typed color (dashed until valid) */}
          <div
            class="w-6 h-6 rounded-full border shrink-0"
            style={{
              "background-color": customValid() ? normalizedCustom()! : "transparent",
              "border-color": "var(--border)",
              "border-style": customValid() ? "solid" : "dashed",
            }}
            aria-hidden="true"
          />
          <input
            type="text"
            value={customHex()}
            placeholder={t("appearance.accent.hexPlaceholder")}
            spellcheck={false}
            class="flex-1 min-w-0 px-2 py-1.5 rounded-lg border text-xs font-mono"
            style={{
              "background-color": "var(--bg)",
              "border-color": showHexError() ? "var(--error)" : "var(--border)",
              color: "var(--text-primary)",
              outline: "none",
            }}
            aria-label={t("appearance.accent.customLabel")}
            aria-invalid={showHexError()}
            onInput={(e) => {
              setTouched(true);
              setCustomHex(e.currentTarget.value);
            }}
            onBlur={() => {
              if (customValid()) commitCustom();
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                commitCustom();
              }
            }}
          />
          <button
            class="px-2.5 py-1.5 rounded-lg text-xs font-medium border shrink-0 transition-colors hover:opacity-80 disabled:opacity-50"
            style={{
              "background-color": "var(--bg)",
              "border-color": "var(--border)",
              color: "var(--text-primary)",
              cursor: customValid() ? "pointer" : "not-allowed",
            }}
            disabled={!customValid()}
            onClick={commitCustom}
          >
            {t("appearance.accent.apply")}
          </button>
        </div>
        <Show when={showHexError()}>
          <p class="text-xs mt-1.5 pl-16" style={{ color: "var(--error)" }}>
            {t("appearance.accent.invalidHex")}
          </p>
        </Show>
      </div>
    </div>
  );
}

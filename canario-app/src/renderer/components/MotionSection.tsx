// Motion section content — master animations toggle + per-effect
// toggles for the visible PRD §8.4 effects, plus the OS
// reduced-motion override notice.
// Pure presentation: AppPage owns the state and the persistence.
import { For, Show } from "solid-js";
import { t, type MessageKey } from "../i18n";
import { Toggle } from "./Toggle";
import {
  ANIMATION_EFFECTS,
  type AnimationEffect,
  type AnimationSettings,
} from "../primitives/animations";

const EFFECT_LABEL_KEYS: Record<AnimationEffect, { name: MessageKey; desc: MessageKey }> = {
  overlay_slide: {
    name: "motion.effect.overlay_slide.name",
    desc: "motion.effect.overlay_slide.desc",
  },
  recording_dot_pulse: {
    name: "motion.effect.recording_dot_pulse.name",
    desc: "motion.effect.recording_dot_pulse.desc",
  },
  toggle_slide: {
    name: "motion.effect.toggle_slide.name",
    desc: "motion.effect.toggle_slide.desc",
  },
  delete_slide: {
    name: "motion.effect.delete_slide.name",
    desc: "motion.effect.delete_slide.desc",
  },
  window_fade: {
    name: "motion.effect.window_fade.name",
    desc: "motion.effect.window_fade.desc",
  },
};

interface Props {
  settings: AnimationSettings;
  /** True while the OS requests reduced motion (overrides everything). */
  reducedMotion: boolean;
  onChange: (next: AnimationSettings) => void;
}

export function MotionSection(props: Props) {
  function setFlag(flag: keyof AnimationSettings, value: boolean) {
    props.onChange({ ...props.settings, [flag]: value });
  }

  return (
    <div class="flex flex-col gap-4">
      {/* Master toggle */}
      <div class="flex items-center justify-between">
        <div>
          <p class="text-sm font-medium">{t("motion.master.title")}</p>
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            {t("motion.master.desc")}
          </p>
        </div>
        <Toggle checked={props.settings.enabled} onChange={(v) => setFlag("enabled", v)} />
      </div>

      {/* OS reduced-motion override */}
      <Show when={props.reducedMotion}>
        <div class="rounded-lg p-3" style={{ "background-color": "var(--surface-hover)" }}>
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            {t("motion.reducedMotion")}
          </p>
        </div>
      </Show>

      {/* Per-effect toggles (only meaningful while the master is on) */}
      <Show when={props.settings.enabled}>
        <div class="flex flex-col gap-3">
          <For each={ANIMATION_EFFECTS}>
            {(effect) => (
              <div class="flex items-center justify-between">
                <div>
                  <p class="text-sm font-medium">{t(EFFECT_LABEL_KEYS[effect].name)}</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                    {t(EFFECT_LABEL_KEYS[effect].desc)}
                  </p>
                </div>
                <Toggle
                  checked={props.settings[effect]}
                  disabled={props.reducedMotion}
                  onChange={(v) => setFlag(effect, v)}
                />
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

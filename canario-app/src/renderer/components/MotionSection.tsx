// Motion section content — master animations toggle + per-effect
// toggles for the visible PRD §8.4 effects, plus the OS
// reduced-motion override notice.
// Pure presentation: AppPage owns the state and the persistence.
import { For, Show } from "solid-js";
import { Toggle } from "./Toggle";
import {
  ANIMATION_EFFECTS,
  type AnimationEffect,
  type AnimationSettings,
} from "../primitives/animations";

const EFFECT_LABELS: Record<AnimationEffect, { name: string; desc: string }> = {
  overlay_slide: {
    name: "Overlay slide-in",
    desc: "Recording island slides down when recording starts",
  },
  recording_dot_pulse: {
    name: "Recording dot pulse",
    desc: "Pulsing red dot while recording",
  },
  toggle_slide: {
    name: "Toggle slide",
    desc: "Switches slide and change color",
  },
  delete_slide: {
    name: "Delete slide-out",
    desc: "History items slide out when deleted",
  },
  window_fade: {
    name: "Window fade-in",
    desc: "Windows fade and scale in when they open",
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
          <p class="text-sm font-medium">Animations</p>
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            Play interface animations
          </p>
        </div>
        <Toggle checked={props.settings.enabled} onChange={(v) => setFlag("enabled", v)} />
      </div>

      {/* OS reduced-motion override */}
      <Show when={props.reducedMotion}>
        <div class="rounded-lg p-3" style={{ "background-color": "var(--surface-hover)" }}>
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            Your system requests reduced motion — animations stay off while that OS
            setting is on, regardless of the toggles here.
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
                  <p class="text-sm font-medium">{EFFECT_LABELS[effect].name}</p>
                  <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                    {EFFECT_LABELS[effect].desc}
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

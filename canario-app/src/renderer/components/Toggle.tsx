// Reusable toggle switch with smooth 200ms CSS transition
// See PRD §8.4 — slide + color change, 200ms ease-in-out
import { Show } from "solid-js";

interface Props {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}

export function Toggle(props: Props) {
  return (
    <button
      role="switch"
      aria-checked={props.checked}
      class="toggle-switch"
      classList={{ "toggle-active": props.checked, "toggle-disabled": !!props.disabled }}
      onClick={() => !props.disabled && props.onChange(!props.checked)}
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          if (!props.disabled) props.onChange(!props.checked);
        }
      }}
    >
      <span class="toggle-thumb" />
      <Show when={props.checked}>
        <span class="toggle-check">✓</span>
      </Show>
    </button>
  );
}

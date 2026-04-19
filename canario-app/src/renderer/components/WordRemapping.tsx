// Word remapping editor — manage find/replace rules and word removals
import { createSignal, For, Show } from "solid-js";

interface Remapping {
  from: string;
  to: string;
}

interface Removal {
  word: string;
}

interface Props {
  remappings: Remapping[];
  removals: Removal[];
  onChange: (postProcessor: { remappings: Remapping[]; removals: Removal[] }) => void;
}

export function WordRemapping(props: Props) {
  const [newFrom, setNewFrom] = createSignal("");
  const [newTo, setNewTo] = createSignal("");
  const [newRemoval, setNewRemoval] = createSignal("");
  const [tab, setTab] = createSignal<"remap" | "remove">("remap");

  function handleAddRemapping() {
    const from = newFrom().trim();
    const to = newTo().trim();
    if (!from || !to) return;

    props.onChange({
      remappings: [...props.remappings, { from, to }],
      removals: props.removals,
    });
    setNewFrom("");
    setNewTo("");
  }

  function handleRemoveRemapping(index: number) {
    const next = [...props.remappings];
    next.splice(index, 1);
    props.onChange({ remappings: next, removals: props.removals });
  }

  function handleAddRemoval() {
    const word = newRemoval().trim();
    if (!word) return;

    props.onChange({
      remappings: props.remappings,
      removals: [...props.removals, { word }],
    });
    setNewRemoval("");
  }

  function handleRemoveRemoval(index: number) {
    const next = [...props.removals];
    next.splice(index, 1);
    props.onChange({ remappings: props.remappings, removals: next });
  }

  const inputStyle = {
    "background-color": "var(--bg)",
    border: "1px solid var(--border)",
    color: "var(--text-primary)",
    "border-radius": "6px",
    padding: "6px 10px",
    "font-size": "13px",
    outline: "none",
    width: "100%",
  };

  return (
    <div>
      {/* Tab switcher */}
      <div class="flex gap-1 mb-3 p-0.5 rounded-lg" style={{ "background-color": "var(--bg)" }}>
        <button
          class="flex-1 px-3 py-1.5 rounded-md text-xs font-medium transition-colors"
          style={{
            "background-color": tab() === "remap" ? "var(--surface-hover)" : "transparent",
            color: tab() === "remap" ? "var(--text-primary)" : "var(--text-secondary)",
            cursor: "pointer",
          }}
          onClick={() => setTab("remap")}
        >
          Find → Replace
        </button>
        <button
          class="flex-1 px-3 py-1.5 rounded-md text-xs font-medium transition-colors"
          style={{
            "background-color": tab() === "remove" ? "var(--surface-hover)" : "transparent",
            color: tab() === "remove" ? "var(--text-primary)" : "var(--text-secondary)",
            cursor: "pointer",
          }}
          onClick={() => setTab("remove")}
        >
          Remove Words
        </button>
      </div>

      <Show when={tab() === "remap"}>
        {/* Existing remappings */}
        <div class="flex flex-col gap-1.5 mb-3">
          <For each={props.remappings}>
            {(rule, i) => (
              <div class="flex items-center gap-2 text-sm">
                <code class="flex-1 px-2 py-1.5 rounded text-xs" style={{ "background-color": "var(--bg)", color: "var(--text-primary)" }}>
                  {rule.from}
                </code>
                <span style={{ color: "var(--text-secondary)" }}>→</span>
                <code class="flex-1 px-2 py-1.5 rounded text-xs" style={{ "background-color": "var(--bg)", color: "var(--text-primary)" }}>
                  {rule.to}
                </code>
                <button
                  class="text-xs px-1.5 py-1 rounded"
                  style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                  onClick={() => handleRemoveRemapping(i())}
                >
                  ✕
                </button>
              </div>
            )}
          </For>
        </div>

        {/* Add new remapping */}
        <div class="flex items-center gap-2">
          <input
            type="text"
            placeholder="Find"
            value={newFrom()}
            onInput={(e) => setNewFrom(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Enter" && handleAddRemapping()}
            style={inputStyle}
          />
          <span style={{ color: "var(--text-secondary)", "font-size": "13px" }}>→</span>
          <input
            type="text"
            placeholder="Replace"
            value={newTo()}
            onInput={(e) => setNewTo(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Enter" && handleAddRemapping()}
            style={inputStyle}
          />
          <button
            class="px-3 py-1.5 rounded-lg text-sm font-medium shrink-0 transition-colors"
            style={{
              "background-color": "var(--accent)",
              color: "white",
              cursor: "pointer",
            }}
            onClick={handleAddRemapping}
            disabled={!newFrom().trim() || !newTo().trim()}
          >
            +
          </button>
        </div>
      </Show>

      <Show when={tab() === "remove"}>
        {/* Existing removals */}
        <div class="flex flex-col gap-1.5 mb-3">
          <For each={props.removals}>
            {(rule, i) => (
              <div class="flex items-center gap-2 text-sm">
                <code class="flex-1 px-2 py-1.5 rounded text-xs" style={{ "background-color": "var(--bg)", color: "var(--text-primary)" }}>
                  {rule.word}
                </code>
                <button
                  class="text-xs px-1.5 py-1 rounded"
                  style={{ color: "var(--text-secondary)", cursor: "pointer" }}
                  onClick={() => handleRemoveRemoval(i())}
                >
                  ✕
                </button>
              </div>
            )}
          </For>
        </div>

        {/* Add new removal */}
        <div class="flex items-center gap-2">
          <input
            type="text"
            placeholder="Word to remove"
            value={newRemoval()}
            onInput={(e) => setNewRemoval(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Enter" && handleAddRemoval()}
            style={inputStyle}
          />
          <button
            class="px-3 py-1.5 rounded-lg text-sm font-medium shrink-0 transition-colors"
            style={{
              "background-color": "var(--accent)",
              color: "white",
              cursor: "pointer",
            }}
            onClick={handleAddRemoval}
            disabled={!newRemoval().trim()}
          >
            +
          </button>
        </div>
      </Show>
    </div>
  );
}

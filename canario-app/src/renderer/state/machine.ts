// Global state machine for Canario app
// Built on SolidJS signals — zero external dependencies
// See PRD §3.4

import { createSignal } from "solid-js";
import type { AppState, AppEvent, AppContext } from "./types";
import { transitions, defaultContext } from "./types";

export function createAppMachine() {
  const [state, setState] = createSignal<AppState>({
    status: "idle",
    hasModel: false,
  });
  const [context, setContext] = createSignal<AppContext>(defaultContext);

  function send(event: AppEvent) {
    const current = state();
    const status = current.status;
    const ctx = context();

    const handler = transitions[status]?.[event.type];
    if (!handler) return; // ignore invalid transitions

    const next = handler(ctx, event);
    if (!next) return; // guard rejected

    setState(next);

    // Update context based on event
    setContext((prev) => {
      const update = { ...prev };
      switch (event.type) {
        case "TRANSCRIPTION_READY":
          // Updated externally by the IPC bridge
          break;
        case "DOWNLOAD_COMPLETE":
          update.modelReady = true;
          break;
        case "DOWNLOAD_FAILED":
          update.modelReady = false;
          break;
        case "ERROR":
          // Error is set externally
          break;
      }
      return update;
    });
  }

  function updateContext(partial: Partial<AppContext>) {
    setContext((prev) => ({ ...prev, ...partial }));
  }

  return { state, context, send, updateContext };
}

export type AppMachine = ReturnType<typeof createAppMachine>;

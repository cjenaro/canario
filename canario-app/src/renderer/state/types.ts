// State machine types for the Canario app
// See PRD §3.4 for the full state graph

export type AppState =
  | { status: "onboarding"; step: number }
  | { status: "idle"; hasModel: boolean }
  | { status: "recording"; startedAt: number }
  | { status: "transcribing"; startedAt: number }
  | { status: "downloading"; progress: number };

export type AppEvent =
  | { type: "WIZARD_COMPLETE" }
  | { type: "START_RECORDING" }
  | { type: "STOP_RECORDING" }
  | { type: "START_DOWNLOAD" }
  | { type: "DOWNLOAD_PROGRESS"; progress: number }
  | { type: "DOWNLOAD_COMPLETE" }
  | { type: "DOWNLOAD_FAILED" }
  | { type: "TRANSCRIPTION_READY" }
  | { type: "RECORDING_STOPPED" }
  | { type: "ERROR" };

export type AppContext = {
  modelReady: boolean;
  lastTranscription: string | null;
  lastError: string | null;
  lastDuration: number | null;
  config: Record<string, unknown> | null;
};

export const defaultContext: AppContext = {
  modelReady: false,
  lastTranscription: null,
  lastError: null,
  lastDuration: null,
  config: null,
};

// Transition map: state → event → next state
// Returns `undefined` to reject the transition.
type TransitionFn = (ctx: AppContext, event: AppEvent) => AppState | undefined;

type TransitionMap = Record<AppState["status"], Partial<Record<AppEvent["type"], TransitionFn>>>;

export const transitions: TransitionMap = {
  onboarding: {
    WIZARD_COMPLETE: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  idle: {
    START_RECORDING: (ctx) => {
      if (!ctx.modelReady) return undefined;
      return { status: "recording", startedAt: Date.now() };
    },
    START_DOWNLOAD: () => ({ status: "downloading", progress: 0 }),
  },
  recording: {
    STOP_RECORDING: () => ({ status: "transcribing", startedAt: Date.now() }),
    ERROR: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  transcribing: {
    TRANSCRIPTION_READY: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    RECORDING_STOPPED: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
    ERROR: (ctx) => ({ status: "idle", hasModel: ctx.modelReady }),
  },
  downloading: {
    DOWNLOAD_PROGRESS: (_ctx, event) => {
      const progress = event.type === "DOWNLOAD_PROGRESS" ? event.progress : 0;
      return { status: "downloading", progress };
    },
    DOWNLOAD_COMPLETE: () => ({ status: "idle", hasModel: true }),
    DOWNLOAD_FAILED: () => ({ status: "idle", hasModel: false }),
  },
};

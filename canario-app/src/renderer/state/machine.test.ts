// Tests for the global app state machine — full transition map
import { describe, it, expect } from "vitest";
import { createAppMachine } from "./machine";
import { defaultContext } from "./types";

describe("createAppMachine", () => {
  it("starts idle with the default context", () => {
    const m = createAppMachine();
    expect(m.state()).toEqual({ status: "idle", hasModel: false });
    expect(m.context()).toEqual(defaultContext);
  });

  describe("onboarding", () => {
    it("WIZARD_GOTO navigates to a valid step", () => {
      const m = createAppMachine();
      m.send({ type: "START_ONBOARDING" });
      expect(m.state()).toEqual({ status: "onboarding", step: 1 });

      m.send({ type: "WIZARD_GOTO", step: 3 });
      expect(m.state()).toEqual({ status: "onboarding", step: 3 });
    });

    it("WIZARD_GOTO rejects out-of-range steps", () => {
      const m = createAppMachine();
      m.send({ type: "START_ONBOARDING" });

      m.send({ type: "WIZARD_GOTO", step: 0 });
      expect(m.state()).toEqual({ status: "onboarding", step: 1 });

      m.send({ type: "WIZARD_GOTO", step: 4 });
      expect(m.state()).toEqual({ status: "onboarding", step: 1 });
    });

    it("WIZARD_COMPLETE goes idle, carrying modelReady into hasModel", () => {
      const m = createAppMachine();
      m.updateContext({ modelReady: true });
      m.send({ type: "START_ONBOARDING" });
      m.send({ type: "WIZARD_COMPLETE" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
    });

    it("START_ONBOARDING is only valid from idle", () => {
      const m = createAppMachine();
      m.send({ type: "START_ONBOARDING" });
      // Already onboarding — a second START_ONBOARDING is a no-op
      m.send({ type: "WIZARD_GOTO", step: 2 });
      m.send({ type: "START_ONBOARDING" });
      expect(m.state()).toEqual({ status: "onboarding", step: 2 });
    });
  });

  describe("idle", () => {
    it("START_RECORDING is rejected until the model is ready", () => {
      const m = createAppMachine();
      m.send({ type: "START_RECORDING" });
      expect(m.state().status).toBe("idle");
    });

    it("START_RECORDING enters recording once the model is ready", () => {
      const m = createAppMachine();
      m.updateContext({ modelReady: true });
      m.send({ type: "START_RECORDING" });
      expect(m.state().status).toBe("recording");
    });

    it("START_DOWNLOAD enters downloading at 0%", () => {
      const m = createAppMachine();
      m.send({ type: "START_DOWNLOAD" });
      expect(m.state()).toEqual({ status: "downloading", progress: 0 });
    });
  });

  describe("recording", () => {
    function recordingMachine() {
      const m = createAppMachine();
      m.updateContext({ modelReady: true });
      m.send({ type: "START_RECORDING" });
      return m;
    }

    it("STOP_RECORDING enters transcribing", () => {
      const m = recordingMachine();
      m.send({ type: "STOP_RECORDING" });
      expect(m.state().status).toBe("transcribing");
    });

    it("RECORDING_CANCELLED returns straight to idle (no transcribing)", () => {
      const m = recordingMachine();
      m.send({ type: "RECORDING_CANCELLED" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
    });

    it("ERROR returns to idle", () => {
      const m = recordingMachine();
      m.send({ type: "ERROR" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
    });

    it("rejects events that make no sense mid-recording", () => {
      const m = recordingMachine();
      for (const event of [
        { type: "TRANSCRIPTION_READY" },
        { type: "RECORDING_STOPPED" },
        { type: "START_RECORDING" },
        { type: "START_ONBOARDING" },
        { type: "WIZARD_COMPLETE" },
      ] as const) {
        m.send(event);
        expect(m.state().status).toBe("recording");
      }
    });
  });

  describe("transcribing", () => {
    function transcribingMachine() {
      const m = createAppMachine();
      m.updateContext({ modelReady: true });
      m.send({ type: "START_RECORDING" });
      m.send({ type: "STOP_RECORDING" });
      return m;
    }

    it("TRANSCRIPTION_READY returns to idle", () => {
      const m = transcribingMachine();
      m.send({ type: "TRANSCRIPTION_READY" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
    });

    it("RECORDING_STOPPED returns to idle (too-short / no-speech end)", () => {
      const m = transcribingMachine();
      m.send({ type: "RECORDING_STOPPED" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
    });

    it("ERROR returns to idle", () => {
      const m = transcribingMachine();
      m.send({ type: "ERROR" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
    });

    it("RECORDING_CANCELLED is a no-op once transcribing", () => {
      const m = transcribingMachine();
      m.send({ type: "RECORDING_CANCELLED" });
      expect(m.state().status).toBe("transcribing");
    });
  });

  describe("downloading", () => {
    it("DOWNLOAD_PROGRESS tracks progress", () => {
      const m = createAppMachine();
      m.send({ type: "START_DOWNLOAD" });
      m.send({ type: "DOWNLOAD_PROGRESS", progress: 42 });
      expect(m.state()).toEqual({ status: "downloading", progress: 42 });
    });

    it("DOWNLOAD_COMPLETE goes idle with the model ready", () => {
      const m = createAppMachine();
      m.send({ type: "START_DOWNLOAD" });
      m.send({ type: "DOWNLOAD_COMPLETE" });
      expect(m.state()).toEqual({ status: "idle", hasModel: true });
      expect(m.context().modelReady).toBe(true);
    });

    it("DOWNLOAD_FAILED goes idle with the model not ready", () => {
      const m = createAppMachine();
      m.send({ type: "START_DOWNLOAD" });
      m.send({ type: "DOWNLOAD_FAILED" });
      expect(m.state()).toEqual({ status: "idle", hasModel: false });
      expect(m.context().modelReady).toBe(false);
    });
  });

  describe("updateContext", () => {
    it("merges partial updates", () => {
      const m = createAppMachine();
      m.updateContext({ lastTranscription: "hello" });
      m.updateContext({ modelReady: true });
      expect(m.context().lastTranscription).toBe("hello");
      expect(m.context().modelReady).toBe(true);
      expect(m.context().lastError).toBeNull();
    });
  });
});

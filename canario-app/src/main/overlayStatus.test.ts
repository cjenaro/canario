// Tests for the stop→result overlay labelling (fgm.4): the push sent
// on a successful stop command is "transforming" only when the cached
// AppConfig's transform block is enabled — absent, malformed, or off
// all keep today's "transcribing" (fgm.1 D5a's safe direction).
import { describe, it, expect } from "vitest";
import { overlayStatusForStop, transformEnabledInConfig } from "./overlayStatus";

describe("transformEnabledInConfig", () => {
  it("reads enabled=true", () => {
    expect(transformEnabledInConfig({ transform: { enabled: true } })).toBe(true);
  });

  it.each([
    ["null config", null],
    ["undefined config", undefined],
    ["no transform block", { auto_paste: true }],
    ["disabled block", { transform: { enabled: false } }],
    ["absent enabled", { transform: { provider: { base_url: "http://localhost:11434/v1" } } }],
    ["truthy-but-not-true", { transform: { enabled: "yes" } }],
    ["malformed block (string)", { transform: "on" }],
    ["malformed block (array)", { transform: [{ enabled: true }] }],
  ])("reads %s as off", (_label, config) => {
    expect(transformEnabledInConfig(config)).toBe(false);
  });
});

describe("overlayStatusForStop", () => {
  it("labels the stop→result window transforming when the pass is enabled", () => {
    expect(
      overlayStatusForStop({ transform: { enabled: true, provider: { base_url: "http://localhost:11434/v1", model: "qwen" } } })
    ).toBe("transforming");
  });

  it("keeps the transcribing label otherwise (today's behavior, D5a)", () => {
    expect(overlayStatusForStop(null)).toBe("transcribing");
    expect(overlayStatusForStop({})).toBe("transcribing");
    expect(overlayStatusForStop({ transform: { enabled: false } })).toBe("transcribing");
  });
});

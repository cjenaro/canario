// parseLegacyOnboardingFile — pure parsing for the one-time
// onboarding.json → AppConfig migration (canario-xv9).
import { describe, expect, it } from "vitest";
import { parseLegacyOnboardingFile } from "./onboarding";

describe("parseLegacyOnboardingFile", () => {
  it("reads a completed flag", () => {
    expect(parseLegacyOnboardingFile('{"completed":true}')).toBe(true);
  });

  it("explicitly-not-completed counts as not completed", () => {
    expect(parseLegacyOnboardingFile('{"completed":false}')).toBe(false);
  });

  it("missing key counts as not completed", () => {
    expect(parseLegacyOnboardingFile("{}")).toBe(false);
    expect(parseLegacyOnboardingFile('{"other":true}')).toBe(false);
  });

  it("null (no legacy file) counts as not completed", () => {
    expect(parseLegacyOnboardingFile(null)).toBe(false);
  });

  it("corrupt JSON counts as not completed", () => {
    expect(parseLegacyOnboardingFile("{ this is not json")).toBe(false);
    expect(parseLegacyOnboardingFile("")).toBe(false);
  });

  it("coerces like the legacy read did (!! on the value)", () => {
    // The pre-migration main-process handler used `!!...completed`, and
    // the migration must preserve the user's observable state exactly —
    // so junk-but-truthy values import as completed, same as before.
    expect(parseLegacyOnboardingFile('{"completed":"yes"}')).toBe(true);
    expect(parseLegacyOnboardingFile('{"completed":0}')).toBe(false);
  });
});

// Tests for the Transformation settings pure logic (canario-fgm.2):
// base-URL validation, loopback-vs-remote warning (incl. the one-time
// localStorage dismissal), the write-only key field semantics, timeout
// clamping, config parsing, and the FULL-block update_config payload
// (whole-key merge — a partial block resets unmentioned subfields, see
// core's transform_apply_as_a_whole_key test).
import { describe, it, expect } from "vitest";
import {
  DEFAULT_TRANSFORM_SETTINGS,
  MAX_TRANSFORM_TIMEOUT_MS,
  MIN_TRANSFORM_TIMEOUT_MS,
  REMOTE_WARNING_DISMISSED_KEY,
  apiKeyPlaceholder,
  baseUrlHostname,
  clampTransformTimeoutMs,
  dismissRemoteWarning,
  isLoopbackBaseUrl,
  isRemoteWarningDismissed,
  isValidBaseUrl,
  keyCommitAction,
  shouldShowTransformFallbackToast,
  shouldWarnRemoteEndpoint,
  transformConfigPayload,
  transformFromConfig,
  transformTestResultFromResponse,
  transcriptionTransformFailed,
  type KeyValueStorage,
} from "./transform";

// ── URL validation ──────────────────────────────────────────────────────────

describe("isValidBaseUrl", () => {
  it.each([
    "https://api.openai.com/v1",
    "http://localhost:11434/v1",
    "http://127.0.0.1:8080/v1",
    "http://[::1]:11434/v1",
    "  https://api.openai.com/v1  ", // trimmed
    "http://localhost/", // trailing slash
  ])("accepts %s", (url) => {
    expect(isValidBaseUrl(url)).toBe(true);
  });

  it.each([
    "",
    "   ",
    "localhost:11434", // parsed as scheme "localhost:" — not http(s)
    "api.openai.com/v1", // relative — not an absolute URL
    "not a url",
    "http://", // empty host
    "ftp://api.openai.com/v1", // scheme outside http(s)
    "file:///etc/passwd",
  ])("rejects %s", (url) => {
    expect(isValidBaseUrl(url)).toBe(false);
  });
});

describe("baseUrlHostname", () => {
  it("returns the host of a valid URL, without IPv6 brackets", () => {
    expect(baseUrlHostname("https://api.openai.com/v1")).toBe("api.openai.com");
    expect(baseUrlHostname("http://[::1]:8080/v1")).toBe("::1");
    expect(baseUrlHostname("http://localhost:11434")).toBe("localhost");
  });

  it("returns null for invalid input", () => {
    expect(baseUrlHostname("not a url")).toBeNull();
    expect(baseUrlHostname("")).toBeNull();
  });
});

// ── Loopback detection (D5c — must mirror canario-core's helper) ────────────

describe("isLoopbackBaseUrl", () => {
  // The same list as canario_core::transform::tests::loopback_detection.
  it.each([
    "http://localhost:11434/v1",
    "https://localhost/v1",
    "http://LOCALHOST:8080/v1", // case-insensitive host
    "http://127.0.0.1:8080/v1",
    "http://127.0.0.1/v1",
    "http://127.1.2.3/v1", // whole 127/8 is loopback
    "http://[::1]:8080/v1",
    "http://[::ffff:127.0.0.1]:8080/v1",
    "http://[::ffff:7f00:1]:8080/v1", // equivalent serialization
    "  http://localhost:11434/v1  ", // trimmed (matches core)
  ])("treats %s as loopback", (url) => {
    expect(isLoopbackBaseUrl(url)).toBe(true);
  });

  it.each([
    "https://api.openai.com/v1",
    "http://api.anthropic.com/v1",
    "http://192.168.1.10:11434/v1", // LAN is not local
    "http://10.0.0.2/v1",
    "http://172.17.0.1/v1",
    "http://0.0.0.0/v1",
    "http://[::2]/v1",
    "http://[::ffff:192.168.1.5]/v1", // IPv4-mapped, non-loopback tail
  ])("treats %s as remote", (url) => {
    expect(isLoopbackBaseUrl(url)).toBe(false);
  });

  it.each([
    "",
    "   ",
    "not a url",
    "localhost:11434",
    "ftp://localhost/v1", // loopback host, but not an http(s) endpoint
  ])("treats %s as conservatively non-local", (url) => {
    expect(isLoopbackBaseUrl(url)).toBe(false);
  });
});

// ── One-time remote warning (D5c) ───────────────────────────────────────────

describe("shouldWarnRemoteEndpoint", () => {
  it("warns for a valid remote URL", () => {
    expect(shouldWarnRemoteEndpoint("https://api.openai.com/v1", false)).toBe(true);
  });

  it("never warns for loopback endpoints", () => {
    expect(shouldWarnRemoteEndpoint("http://localhost:11434/v1", false)).toBe(false);
    expect(shouldWarnRemoteEndpoint("http://127.0.0.1:8080/v1", false)).toBe(false);
    expect(shouldWarnRemoteEndpoint("http://[::1]:11434/v1", false)).toBe(false);
  });

  it("does not warn for an invalid URL (the inline validation error owns that state)", () => {
    expect(shouldWarnRemoteEndpoint("api.openai.com/v1", false)).toBe(false);
    expect(shouldWarnRemoteEndpoint("localhost:11434", false)).toBe(false);
    expect(shouldWarnRemoteEndpoint("", false)).toBe(false);
  });

  it("stays dismissed after the one-time acknowledgement", () => {
    expect(shouldWarnRemoteEndpoint("https://api.openai.com/v1", true)).toBe(false);
  });
});

/** Minimal localStorage stand-in. */
function fakeStorage(initial: Record<string, string> = {}): KeyValueStorage {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
    setItem: (key: string, value: string) => void map.set(key, value),
  };
}

describe("remote warning dismissal storage", () => {
  it("is not dismissed by default", () => {
    expect(isRemoteWarningDismissed(fakeStorage())).toBe(false);
  });

  it("round-trips a dismissal under the documented key", () => {
    const storage = fakeStorage();
    expect(isRemoteWarningDismissed(storage)).toBe(false);

    dismissRemoteWarning(storage);

    expect(isRemoteWarningDismissed(storage)).toBe(true);
    // Key pinned: the flag is the app's own namespace, not an accident.
    expect(storage.getItem(REMOTE_WARNING_DISMISSED_KEY)).toBe("1");
  });

  it("ignores a foreign value under the key (only an explicit dismissal counts)", () => {
    expect(isRemoteWarningDismissed(fakeStorage({ [REMOTE_WARNING_DISMISSED_KEY]: "0" }))).toBe(
      false,
    );
  });

  it("degrades gracefully when storage throws (blocked storage)", () => {
    const throwing: KeyValueStorage = {
      getItem: () => {
        throw new Error("SecurityError");
      },
      setItem: () => {
        throw new Error("SecurityError");
      },
    };
    expect(isRemoteWarningDismissed(throwing)).toBe(false);
    expect(() => dismissRemoteWarning(throwing)).not.toThrow();
  });
});

// ── Write-only key field (D2) ───────────────────────────────────────────────

describe("keyCommitAction", () => {
  it("stores any non-empty value", () => {
    expect(keyCommitAction("sk-live-123", false)).toBe("store");
    expect(keyCommitAction("sk-live-123", true)).toBe("store");
    expect(keyCommitAction("  sk-live-123  ", true)).toBe("store");
  });

  it("emptied field with a stored key is the clear path", () => {
    expect(keyCommitAction("", true)).toBe("clear");
    expect(keyCommitAction("   ", true)).toBe("clear");
  });

  it("untouched/emptied field with NO stored key is a no-op", () => {
    // The field always starts empty (the key is never read back), so an
    // empty value only means "clear" when a key exists to clear.
    expect(keyCommitAction("", false)).toBe("noop");
    expect(keyCommitAction("    ", false)).toBe("noop");
  });
});

describe("apiKeyPlaceholder", () => {
  it("explains both write-only states", () => {
    expect(apiKeyPlaceholder(true)).toMatch(/saved/i);
    expect(apiKeyPlaceholder(false)).toMatch(/local/i);
  });
});

// ── Timeout clamp (D5d — mirror of core's effective_timeout window) ─────────

describe("clampTransformTimeoutMs", () => {
  it("falls back to the 4s default for 0 / NaN", () => {
    expect(clampTransformTimeoutMs(0)).toBe(4000);
    expect(clampTransformTimeoutMs(Number.NaN)).toBe(4000);
  });

  it("clamps into [250, 60000]", () => {
    expect(clampTransformTimeoutMs(1)).toBe(MIN_TRANSFORM_TIMEOUT_MS);
    expect(clampTransformTimeoutMs(100)).toBe(250);
    expect(clampTransformTimeoutMs(4000)).toBe(4000);
    expect(clampTransformTimeoutMs(999999)).toBe(MAX_TRANSFORM_TIMEOUT_MS);
  });

  it("rounds to whole milliseconds", () => {
    expect(clampTransformTimeoutMs(1500.6)).toBe(1501);
  });
});

// ── Config parsing ──────────────────────────────────────────────────────────

describe("transformFromConfig", () => {
  it("returns the disabled default when the block is absent", () => {
    expect(transformFromConfig({})).toEqual(DEFAULT_TRANSFORM_SETTINGS);
    expect(transformFromConfig(null)).toEqual(DEFAULT_TRANSFORM_SETTINGS);
  });

  it("returns the disabled default for a malformed block", () => {
    expect(transformFromConfig({ transform: "nope" })).toEqual(DEFAULT_TRANSFORM_SETTINGS);
    expect(transformFromConfig({ transform: [] })).toEqual(DEFAULT_TRANSFORM_SETTINGS);
  });

  it("parses a full block, rules untouched", () => {
    const rules = [{ app: "vim", style: "concise" }];
    const settings = transformFromConfig({
      transform: {
        enabled: true,
        provider: { base_url: "http://localhost:11434/v1", model: "llama3" },
        timeout_ms: 1500,
        rules,
      },
    });
    expect(settings).toEqual({
      enabled: true,
      provider: { base_url: "http://localhost:11434/v1", model: "llama3" },
      timeout_ms: 1500,
      rules,
    });
  });

  it("fills defaults for partial subfields (mirrors core's serde defaults)", () => {
    const settings = transformFromConfig({ transform: { enabled: true } });
    expect(settings.enabled).toBe(true);
    expect(settings.provider).toEqual({ base_url: "", model: "" });
    expect(settings.timeout_ms).toBe(4000);
    expect(settings.rules).toEqual([]);
  });

  it("ignores non-boolean enabled / non-numeric timeout (safe direction: off + default)", () => {
    const settings = transformFromConfig({
      transform: { enabled: "yes", timeout_ms: "slow", provider: "x" },
    });
    expect(settings.enabled).toBe(false);
    expect(settings.timeout_ms).toBe(4000);
    expect(settings.provider).toEqual({ base_url: "", model: "" });
  });
});

// ── update_config payload (whole-key merge semantics) ───────────────────────

describe("transformConfigPayload", () => {
  it("sends the FULL block — every key the sidecar's TransformSettings knows", () => {
    const payload = transformConfigPayload({
      enabled: true,
      provider: { base_url: "https://api.openai.com/v1", model: "gpt-4o-mini" },
      timeout_ms: 2500,
      rules: [{ app: "vim" }],
    });
    expect(payload).toEqual({
      transform: {
        enabled: true,
        provider: { base_url: "https://api.openai.com/v1", model: "gpt-4o-mini" },
        timeout_ms: 2500,
        rules: [{ app: "vim" }],
      },
    });
  });

  it("preserves sibling fields + rules when one field changes (no partial resets)", () => {
    const base = transformFromConfig({
      transform: {
        enabled: true,
        provider: { base_url: "http://localhost:8080/v1", model: "qwen" },
        timeout_ms: 4000,
        rules: [{ app: "vim" }],
      },
    });
    const payload = transformConfigPayload({ ...base, enabled: false });

    expect(payload.transform).toEqual({
      enabled: false,
      provider: { base_url: "http://localhost:8080/v1", model: "qwen" },
      timeout_ms: 4000,
      rules: [{ app: "vim" }],
    });
  });

  it("carries NO key material — the key never travels through config (D2)", () => {
    const payload = transformConfigPayload({
      enabled: true,
      provider: { base_url: "https://api.openai.com/v1", model: "gpt-4o-mini" },
      timeout_ms: 4000,
      rules: [],
    });
    const json = JSON.stringify(payload);
    expect(json).not.toMatch(/sk-|api[-_]?key|credential|bearer/i);
    expect(payload.transform.provider).not.toHaveProperty("key");
  });

  it("defensively clamps the timeout in the payload", () => {
    const payload = transformConfigPayload({
      enabled: false,
      provider: { base_url: "", model: "" },
      timeout_ms: 0,
      rules: [],
    });
    expect(payload.transform.timeout_ms).toBe(4000);
  });
});

// ── transform_test response reduction ───────────────────────────────────────

describe("transformTestResultFromResponse", () => {
  it("reduces a successful probe to its latency", () => {
    expect(
      transformTestResultFromResponse({ ok: true, data: { latency_ms: 342 } }),
    ).toEqual({ phase: "ok", latencyMs: 342 });
  });

  it("rejects a malformed success payload", () => {
    expect(transformTestResultFromResponse({ ok: true })).toEqual({
      phase: "error",
      message: "Unexpected response from the speech backend",
    });
    expect(transformTestResultFromResponse({ ok: true, data: { latency_ms: "fast" } })).toMatchObject({
      phase: "error",
    });
  });

  it("surfaces the sidecar's error message", () => {
    expect(
      transformTestResultFromResponse({ ok: false, error: "HTTP 401 Unauthorized" }),
    ).toEqual({ phase: "error", message: "HTTP 401 Unauthorized" });
  });

  it("falls back to a generic error for a missing/blank reason", () => {
    expect(transformTestResultFromResponse({ ok: false })).toEqual({
      phase: "error",
      message: "Connection test failed",
    });
    expect(transformTestResultFromResponse({ ok: false, error: "   " })).toEqual({
      phase: "error",
      message: "Connection test failed",
    });
  });

  it("maps a null response (bridge/sidecar down) to a backend-reachability error", () => {
    expect(transformTestResultFromResponse(null)).toEqual({
      phase: "error",
      message: "Could not reach the speech backend",
    });
  });
});

// ── TranscriptionReady transform-failure signal (fgm.4) ─────────────────────

describe("transcriptionTransformFailed", () => {
  it("reads field absence as no failure (every event from today's sidecar)", () => {
    expect(
      transcriptionTransformFailed({ event: "TranscriptionReady", text: "hi", duration_secs: 1.5 }),
    ).toBe(false);
    expect(transcriptionTransformFailed({})).toBe(false);
  });

  it.each([
    ["boolean flag (the D5d wording)", { transform_failed: true }],
    ["non-empty error string", { transform_error: "request timed out" }],
  ])("detects the %s shape", (_label, event) => {
    expect(transcriptionTransformFailed(event)).toBe(true);
  });

  it.each([
    ["false flag", { transform_failed: false }],
    ["empty error", { transform_error: "" }],
    ["whitespace error", { transform_error: "   " }],
    ["non-string error", { transform_error: 500 }],
  ])("ignores the %s shape", (_label, event) => {
    expect(transcriptionTransformFailed(event)).toBe(false);
  });
});

describe("shouldShowTransformFallbackToast", () => {
  const failed = { event: "TranscriptionReady", text: "raw words", transform_failed: true };
  const clean = { event: "TranscriptionReady", text: "words" };

  it("shows only when the pass was enabled AND the event signals failure", () => {
    expect(shouldShowTransformFallbackToast(true, failed)).toBe(true);
  });

  it("stays silent when the pass was off (the user never opted in)", () => {
    expect(shouldShowTransformFallbackToast(false, failed)).toBe(false);
  });

  it("stays silent when the event carries no failure signal", () => {
    expect(shouldShowTransformFallbackToast(true, clean)).toBe(false);
    expect(shouldShowTransformFallbackToast(false, clean)).toBe(false);
  });
});

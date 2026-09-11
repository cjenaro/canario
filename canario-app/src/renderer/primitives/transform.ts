// Transformation provider settings — pure logic for the Settings
// "Transformation" section (canario-fgm.2; binding decisions from
// canario-fgm.1: default-off D5a, write-only credential D2, loopback
// first-class D5c).
//
// No DOM, no IPC — TransformSection/AppPage own those so this module
// stays node-testable (mirrors primitives/animations.ts).
//
// Loopback semantics deliberately reimplement
// canario_core::transform::is_loopback_base_url (the renderer cannot
// link Rust): an http/https host that is `localhost`
// (case-insensitive), any 127.0.0.0/8 IPv4, `::1`, or an IPv4-mapped
// `::ffff:127.x.x.x`. Unparseable input is NOT local — the caller
// shows the validation error instead of silently treating a typo as
// safe (same posture as the core helper).

/** Mirrors core's DEFAULT_TRANSFORM_TIMEOUT_MS (fgm.1 D5d). */
export const DEFAULT_TRANSFORM_TIMEOUT_MS = 4000;
/** Mirrors core's MIN_TRANSFORM_TIMEOUT_MS clamp. */
export const MIN_TRANSFORM_TIMEOUT_MS = 250;
/** Mirrors core's MAX_TRANSFORM_TIMEOUT_MS clamp. */
export const MAX_TRANSFORM_TIMEOUT_MS = 60000;

/** Wire shape of AppConfig.transform (snake_case, mirrored in core's TransformSettings). */
export interface TransformProviderSettings {
  /** Base URL including any version path, e.g. `http://localhost:11434/v1`. */
  base_url: string;
  /** Model name sent in the chat-completions payload. */
  model: string;
}

export interface TransformSettings {
  /** Master switch — default OFF (D5a): nothing leaves the machine. */
  enabled: boolean;
  /** OpenAI-compatible endpoint metadata. NEVER includes key material (D2). */
  provider: TransformProviderSettings;
  /** Per-request timeout in ms (D5d). */
  timeout_ms: number;
  /**
   * Per-app rules placeholder — fgm.3 owns the shape, so entries are
   * kept as raw JSON that round-trips untouched (same as core).
   */
  rules: unknown[];
}

export const DEFAULT_TRANSFORM_SETTINGS: TransformSettings = {
  enabled: false,
  provider: { base_url: "", model: "" },
  timeout_ms: DEFAULT_TRANSFORM_TIMEOUT_MS,
  rules: [],
};

/** Mirror core's serde defaults: missing/malformed values fall back. */
function readNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/**
 * Extract + validate the transform block from a get_config payload.
 * Lenient like animationsFromConfig — corrupt/stale data degrades to
 * defaults (which keep the feature OFF, the safe direction) instead of
 * throwing.
 */
export function transformFromConfig(config: unknown): TransformSettings {
  const cfg = (config ?? {}) as Record<string, unknown>;
  const raw = cfg.transform;
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    return { ...DEFAULT_TRANSFORM_SETTINGS, provider: { ...DEFAULT_TRANSFORM_SETTINGS.provider } };
  }
  const block = raw as Record<string, unknown>;
  const rawProvider = block.provider;
  const provider: TransformProviderSettings =
    typeof rawProvider === "object" && rawProvider !== null && !Array.isArray(rawProvider)
      ? {
          base_url:
            typeof (rawProvider as Record<string, unknown>).base_url === "string"
              ? ((rawProvider as Record<string, unknown>).base_url as string)
              : "",
          model:
            typeof (rawProvider as Record<string, unknown>).model === "string"
              ? ((rawProvider as Record<string, unknown>).model as string)
              : "",
        }
      : { ...DEFAULT_TRANSFORM_SETTINGS.provider };
  return {
    enabled: block.enabled === true,
    provider,
    timeout_ms: readNumber(block.timeout_ms, DEFAULT_TRANSFORM_TIMEOUT_MS),
    rules: Array.isArray(block.rules) ? [...block.rules] : [],
  };
}

/**
 * Clamp a timeout into core's sane window (mirrors
 * TransformSettings::effective_timeout): 0/NaN → the 4 s default (an
 * instant timeout would fail every request), then clamped into
 * [250 ms, 60 s] so dictation is never wedged waiting on a provider.
 */
export function clampTransformTimeoutMs(value: number): number {
  const ms =
    !Number.isFinite(value) || value === 0 ? DEFAULT_TRANSFORM_TIMEOUT_MS : value;
  return Math.min(MAX_TRANSFORM_TIMEOUT_MS, Math.max(MIN_TRANSFORM_TIMEOUT_MS, Math.round(ms)));
}

// ── URL validation ──────────────────────────────────────────────────────────

/**
 * Is `raw` a usable provider base URL? Must parse as an absolute URL
 * with an http/https scheme and a host. `localhost:11434` (parsed as
 * scheme `localhost:`) and bare `api.openai.com/v1` are rejected — the
 * inline validation error asks for the full `http(s)://` form.
 */
export function isValidBaseUrl(raw: string): boolean {
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return false;
  }
  return (url.protocol === "http:" || url.protocol === "https:") && url.hostname !== "";
}

/** Hostname of a valid base URL (brackets stripped for IPv6), or null. */
export function baseUrlHostname(raw: string): string | null {
  try {
    const url = new URL(raw.trim());
    return url.hostname ? url.hostname.replace(/^\[|\]$/g, "") : null;
  } catch {
    return null;
  }
}

// ── Loopback detection (D5c — local endpoints are first-class) ──────────────

const IPV4_PATTERN = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/;

function parseIpv4Octets(host: string): number[] | null {
  const match = IPV4_PATTERN.exec(host);
  if (!match) return null;
  const octets = match.slice(1).map(Number);
  return octets.every((o) => Number.isInteger(o) && o >= 0 && o <= 255) ? octets : null;
}

/** Whole 127.0.0.0/8 counts (`127.1.2.3` too), like Ipv4Addr::is_loopback. */
function isLoopbackIpv4(octets: number[]): boolean {
  return octets[0] === 127;
}

/**
 * Parse a bracket-less IPv6 literal into its 8 groups, or null.
 * Handles a single `::` compression (WHATWG-serialized hostnames always
 * use the canonical compression, but stay tolerant of hand-written
 * full forms).
 */
function parseIpv6Groups(host: string): number[] | null {
  if (!/^[0-9a-f:.]+$/.test(host)) return null;
  const halves = host.split("::");
  if (halves.length > 2) return null;
  const parseGroups = (part: string): number[] | null => {
    if (part === "") return [];
    const groups: number[] = [];
    for (const piece of part.split(":")) {
      if (!/^[0-9a-f]{1,4}$/.test(piece)) return null;
      groups.push(parseInt(piece, 16));
    }
    return groups;
  };
  if (halves.length === 1) {
    const groups = parseGroups(halves[0]);
    return groups !== null && groups.length === 8 ? groups : null;
  }
  const head = parseGroups(halves[0]);
  const tail = parseGroups(halves[1]);
  if (head === null || tail === null || head.length + tail.length > 7) return null;
  const filler = new Array<number>(8 - head.length - tail.length).fill(0);
  return [...head, ...filler, ...tail];
}

function isLoopbackIpv6(groups: number[]): boolean {
  // `::1` — every form with zeros except a final 1.
  if (groups.slice(0, 7).every((g) => g === 0) && groups[7] === 1) return true;
  // IPv4-mapped `::ffff:127.x.x.x` — mirror Rust's to_ipv4_mapped check
  // (the WHATWG URL parser normalizes dotted tails to hex groups).
  if (groups.slice(0, 5).every((g) => g === 0) && groups[5] === 0xffff) {
    const octets = [
      (groups[6] >> 8) & 0xff,
      groups[6] & 0xff,
      (groups[7] >> 8) & 0xff,
      groups[7] & 0xff,
    ];
    return isLoopbackIpv4(octets);
  }
  return false;
}

/**
 * Loopback endpoint per fgm.1 D5c — same verdicts as
 * canario_core::transform::is_loopback_base_url for every case either
 * side tests (localhost any case/port, whole 127/8, ::1,
 * ::ffff:127.x.x.x in dotted or hex serialization; LAN addresses,
 * 0.0.0.0, other schemes and unparseable input are NOT local).
 */
export function isLoopbackBaseUrl(raw: string): boolean {
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return false;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return false;
  // WHATWG hostname keeps brackets on IPv6 literals and lowercases hosts.
  const host = url.hostname.replace(/^\[|\]$/g, "");
  if (host === "localhost") return true;
  const octets = parseIpv4Octets(host);
  if (octets) return isLoopbackIpv4(octets);
  const groups = parseIpv6Groups(host);
  if (groups) return isLoopbackIpv6(groups);
  return false;
}

// ── One-time remote-endpoint warning (D5c) ──────────────────────────────────

/** localStorage flag recording the user's "don't show again" choice. */
export const REMOTE_WARNING_DISMISSED_KEY = "canario.transform.remote-warning-dismissed";

/** The slice of localStorage the helpers need (testable without a DOM). */
export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function isRemoteWarningDismissed(storage: KeyValueStorage): boolean {
  try {
    return storage.getItem(REMOTE_WARNING_DISMISSED_KEY) === "1";
  } catch {
    // Storage unavailable (blocked/disabled) — keep showing the warning.
    return false;
  }
}

export function dismissRemoteWarning(storage: KeyValueStorage): void {
  try {
    storage.setItem(REMOTE_WARNING_DISMISSED_KEY, "1");
  } catch {
    // Storage unavailable — the dismissal holds for this session only.
  }
}

/**
 * Should the one-time non-loopback warning show? Only a VALID remote
 * URL warns: an invalid URL gets the inline validation error instead
 * (two overlapping warnings would be noise), and loopback endpoints
 * are first-class local (D5c). One-time = dismissed globally.
 */
export function shouldWarnRemoteEndpoint(baseUrl: string, dismissed: boolean): boolean {
  return !dismissed && isValidBaseUrl(baseUrl) && !isLoopbackBaseUrl(baseUrl);
}

// ── Write-only API key field (D2) ───────────────────────────────────────────

/**
 * The commit action for an edit of the write-only key field. The key
 * is NEVER read back: the field starts empty whether or not one is
 * stored, and a native `change` (blur) only fires when the user
 * actually edited it — so an untouched empty field is a no-op, an
 * EMPTIED field is the documented clear path, and any non-empty value
 * stores (trimmed main-side).
 */
export function keyCommitAction(
  value: string,
  credentialPresent: boolean,
): "store" | "clear" | "noop" {
  if (value.trim().length > 0) return "store";
  return credentialPresent ? "clear" : "noop";
}

/** Placeholder-copy defaults (English) — i18n callers override via the
 *  `labels` parameter; defaults keep this module's node tests standalone. */
export interface ApiKeyPlaceholderLabels {
  present: string;
  absent: string;
}

export const DEFAULT_API_KEY_PLACEHOLDER_LABELS: ApiKeyPlaceholderLabels = {
  present: "API key saved — type to replace, clear + unfocus to remove",
  absent: "sk-… (not needed for local endpoints)",
};

/** Placeholder copy explaining the write-only field's semantics. */
export function apiKeyPlaceholder(
  credentialPresent: boolean,
  labels: ApiKeyPlaceholderLabels = DEFAULT_API_KEY_PLACEHOLDER_LABELS,
): string {
  return credentialPresent ? labels.present : labels.absent;
}

// ── update_config payload (whole-key merge semantics) ───────────────────────

/**
 * update_config payload for a settings change. The sidecar's merge
 * replaces top-level keys wholesale, so the FULL block must travel
 * with every update — a partial block would reset unmentioned
 * subfields (including `rules`, which fgm.3 will own) to their
 * defaults (see core's transform_apply_as_a_whole_key test). The
 * timeout is defensively clamped so a stale settings object can never
 * persist a value outside the D5d window.
 */
export function transformConfigPayload(settings: TransformSettings): {
  transform: TransformSettings;
} {
  return {
    transform: {
      enabled: settings.enabled === true,
      provider: {
        base_url: settings.provider.base_url,
        model: settings.provider.model,
      },
      timeout_ms: clampTransformTimeoutMs(settings.timeout_ms),
      rules: [...settings.rules],
    },
  };
}

// ── TranscriptionReady transform-failure signal (fgm.4) ─────────────────────

/**
 * Does a TranscriptionReady event carry a transform-failure signal?
 *
 * fgm.1 D5d fixes the CONTRACT — on timeout or ANY transform error the
 * sidecar still emits TranscriptionReady carrying the raw text plus a
 * failure flag, so dictation never blocks or loses audio — while
 * canario-fgm.3 (parallel, core-side) owns the exact wire shape. Coded
 * against, in order:
 *   - `transform_failed: true` — the D5d wording, the expected field
 *   - `transform_error: "<non-empty>"` — the string variant
 * Anything else — crucially the field being ABSENT, which is every
 * event from today's sidecar — reads as "no failure", so the fallback
 * toast hook stays dormant until the field actually appears.
 */
export function transcriptionTransformFailed(event: Record<string, unknown>): boolean {
  if (event.transform_failed === true) return true;
  return typeof event.transform_error === "string" && event.transform_error.trim().length > 0;
}

/**
 * Should the settings window show its one "fell back to raw" toast?
 * Both halves are required (fgm.4): the transform pass was enabled
 * (the user opted in — a failure of a pass they never asked for is
 * silent by design, and per D5d the raw paste already happened) AND
 * the event signals the failure.
 */
export function shouldShowTransformFallbackToast(
  transformEnabled: boolean,
  event: Record<string, unknown>,
): boolean {
  return transformEnabled === true && transcriptionTransformFailed(event);
}

// ── transform_test response reduction ───────────────────────────────────────

/** Display state for the section's "Test connection" button. */
export type TransformTestState =
  | { phase: "idle" }
  | { phase: "running" }
  | { phase: "ok"; latencyMs: number }
  | { phase: "error"; message: string };

/**
 * Renderer-authored fallback strings for a failed/unreachable probe.
 * Sidecar-authored `error` strings pass through untranslated (they come
 * from the Rust backend); these defaults keep the module's node tests
 * standalone — i18n callers override via the `fallbacks` parameter.
 */
export interface TransformTestFallbacks {
  unreachable: string;
  unexpected: string;
  failed: string;
}

export const DEFAULT_TRANSFORM_TEST_FALLBACKS: TransformTestFallbacks = {
  unreachable: "Could not reach the speech backend",
  unexpected: "Unexpected response from the speech backend",
  failed: "Connection test failed",
};

/**
 * Reduce a `transform_test` sidecar response (or a null from a dead
 * backend / bridge failure) into the display state. Error strings are
 * the sidecar's — they carry no key material (the credential only ever
 * travels inside the Authorization header).
 */
export function transformTestResultFromResponse(
  res: Record<string, unknown> | null | undefined,
  fallbacks: TransformTestFallbacks = DEFAULT_TRANSFORM_TEST_FALLBACKS,
): TransformTestState {
  if (!res) {
    return { phase: "error", message: fallbacks.unreachable };
  }
  if (res.ok === true) {
    const latency = (res.data as { latency_ms?: unknown } | undefined)?.latency_ms;
    return typeof latency === "number" && Number.isFinite(latency) && latency >= 0
      ? { phase: "ok", latencyMs: latency }
      : { phase: "error", message: fallbacks.unexpected };
  }
  const message =
    typeof res.error === "string" && res.error.trim().length > 0
      ? res.error
      : fallbacks.failed;
  return { phase: "error", message };
}

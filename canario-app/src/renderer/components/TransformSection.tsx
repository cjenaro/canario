// Transformation section content — BYOK LLM post-processing provider
// (canario-fgm.2; decisions D2/D5 from canario-fgm.1).
//
// Pure presentation: AppPage owns the state and the persistence (the
// FULL transform block via update_config), the credential round-trip
// (write-only key → main-process safeStorage → sidecar memory) and the
// connection probe (sidecar transform_test). All decision logic lives
// in primitives/transform.ts.
//
// D5a: the master toggle defaults to OFF — with it off (and with the
// whole block absent) dictation stays fully on-device, byte-identical
// to a Canario without this feature.
import { createEffect, createSignal, Show } from "solid-js";
import { t } from "../i18n";
import { Toggle } from "./Toggle";
import {
  apiKeyPlaceholder,
  baseUrlHostname,
  clampTransformTimeoutMs,
  dismissRemoteWarning,
  isRemoteWarningDismissed,
  isValidBaseUrl,
  keyCommitAction,
  MAX_TRANSFORM_TIMEOUT_MS,
  MIN_TRANSFORM_TIMEOUT_MS,
  shouldWarnRemoteEndpoint,
  type ApiKeyPlaceholderLabels,
  type TransformSettings,
  type TransformTestState,
} from "../primitives/transform";

/** Write-only-field placeholder copy from the i18n catalog (see
 *  primitives/transform.ts for why the primitive takes it as a parameter). */
const apiKeyLabels = (): ApiKeyPlaceholderLabels => ({
  present: t("transform.apiKey.placeholderPresent"),
  absent: t("transform.apiKey.placeholderAbsent"),
});

interface Props {
  settings: TransformSettings;
  /** The sidecar holds an API key (transform_status.credential_present). */
  credentialPresent: boolean;
  /** Persist new settings — receives the FULL block (AppPage builds the payload). */
  onSettingsChange: (next: TransformSettings) => void;
  /** The user committed an edit of the write-only key field. */
  onKeyCommit: (action: "store" | "clear", key: string) => void;
  /** Run transform_test; this component owns the running/ok/error display. */
  onTest: () => Promise<TransformTestState>;
}

export function TransformSection(props: Props) {
  // Invalid base URL: shown inline, never persisted (the previous
  // valid config stays in force until the user fixes the input).
  const [invalidBaseUrl, setInvalidBaseUrl] = createSignal(false);
  // Write-only key field: starts empty whether or not a key is stored
  // (D2 — the key is never read back) and is wiped after each commit.
  const [keyDraft, setKeyDraft] = createSignal("");
  const [testState, setTestState] = createSignal<TransformTestState>({ phase: "idle" });
  const [warningDismissed, setWarningDismissed] = createSignal(readDismissed());

  function readDismissed(): boolean {
    try {
      return isRemoteWarningDismissed(localStorage);
    } catch {
      return false; // storage blocked — the warning shows each session
    }
  }

  function dismissWarning() {
    try {
      dismissRemoteWarning(localStorage);
    } catch {
      /* session-only dismissal */
    }
    setWarningDismissed(true);
  }

  // A new endpoint or model invalidates any previous probe result.
  createEffect(() => {
    props.settings.provider.base_url;
    props.settings.provider.model;
    setTestState({ phase: "idle" });
  });

  function setEnabled(enabled: boolean) {
    props.onSettingsChange({ ...props.settings, enabled });
  }

  function commitBaseUrl(raw: string) {
    const value = raw.trim();
    if (value.length === 0) {
      setInvalidBaseUrl(false);
      if (props.settings.provider.base_url !== "") {
        props.onSettingsChange({
          ...props.settings,
          provider: { ...props.settings.provider, base_url: "" },
        });
      }
      return;
    }
    if (!isValidBaseUrl(value)) {
      setInvalidBaseUrl(true); // keep the last valid config; explain inline
      return;
    }
    setInvalidBaseUrl(false);
    if (value !== props.settings.provider.base_url) {
      props.onSettingsChange({
        ...props.settings,
        provider: { ...props.settings.provider, base_url: value },
      });
    }
  }

  function commitModel(raw: string) {
    const value = raw.trim();
    if (value === props.settings.provider.model) return;
    props.onSettingsChange({
      ...props.settings,
      provider: { ...props.settings.provider, model: value },
    });
  }

  function commitTimeout(raw: string) {
    const parsed = parseFloat(raw);
    const clamped = clampTransformTimeoutMs(Number.isFinite(parsed) ? parsed : 0);
    if (clamped === props.settings.timeout_ms) return;
    props.onSettingsChange({ ...props.settings, timeout_ms: clamped });
  }

  // change fires only on real edits (blur after typing) — an untouched
  // empty field never commits, an EMPTIED field is the clear path.
  function commitKey(raw: string) {
    const action = keyCommitAction(raw, props.credentialPresent);
    setKeyDraft("");
    if (action === "noop") return;
    props.onKeyCommit(action, action === "store" ? raw : "");
  }

  async function handleTest() {
    if (testState().phase === "running") return;
    setTestState({ phase: "running" });
    try {
      setTestState(await props.onTest());
    } catch {
      setTestState({ phase: "error", message: t("transform.test.failed") });
    }
  }

  const inputStyle = {
    "background-color": "var(--bg)",
    "border-color": "var(--border)",
    color: "var(--text-primary)",
    outline: "none",
  } as const;

  const testable = () =>
    props.settings.provider.base_url.length > 0 && isValidBaseUrl(props.settings.provider.base_url);

  // Narrowed views of the probe state (Show can't narrow signal calls).
  const testOk = () => {
    const t = testState();
    return t.phase === "ok" ? t : null;
  };
  const testError = () => {
    const t = testState();
    return t.phase === "error" ? t : null;
  };

  return (
    <div class="flex flex-col gap-4">
      {/* Master toggle — D5a: OFF by default, everything below hidden */}
      <div class="flex items-center justify-between">
        <div>
          <p class="text-sm font-medium">{t("transform.enable.title")}</p>
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            {t("transform.enable.desc")}
          </p>
        </div>
        <Toggle checked={props.settings.enabled} onChange={setEnabled} />
      </div>

      <Show when={props.settings.enabled}>
        <div class="flex flex-col gap-4">
          {/* Base URL */}
          <div>
            <div class="flex items-center justify-between mb-1">
              <p class="text-sm font-medium">{t("transform.baseUrl.title")}</p>
              <Show when={invalidBaseUrl()}>
                <span class="text-xs" style={{ color: "var(--error)" }}>
                  {t("transform.baseUrl.invalid")}
                </span>
              </Show>
            </div>
            <input
              type="text"
              value={props.settings.provider.base_url}
              placeholder={t("transform.baseUrl.placeholder")}
              onChange={(e) => commitBaseUrl(e.currentTarget.value)}
              class="w-full px-3 py-1.5 rounded-lg border text-sm"
              style={invalidBaseUrl() ? { ...inputStyle, "border-color": "var(--error)" } : inputStyle}
            />
            <p class="text-xs mt-1" style={{ color: "var(--text-secondary)" }}>
              {t("transform.baseUrl.hint")}
            </p>
          </div>

          {/* Model */}
          <div>
            <p class="text-sm font-medium mb-1">{t("transform.model.title")}</p>
            <input
              type="text"
              value={props.settings.provider.model}
              placeholder={t("transform.model.placeholder")}
              onChange={(e) => commitModel(e.currentTarget.value)}
              class="w-full px-3 py-1.5 rounded-lg border text-sm"
              style={inputStyle}
            />
          </div>

          {/* API key — write-only (D2) */}
          <div>
            <p class="text-sm font-medium mb-1">{t("transform.apiKey.title")}</p>
            <input
              type="password"
              value={keyDraft()}
              placeholder={apiKeyPlaceholder(props.credentialPresent, apiKeyLabels())}
              autocomplete="off"
              onChange={(e) => commitKey(e.currentTarget.value)}
              class="w-full px-3 py-1.5 rounded-lg border text-sm"
              style={inputStyle}
            />
            <p class="text-xs mt-1" style={{ color: props.credentialPresent ? "var(--success)" : "var(--text-secondary)" }}>
              {props.credentialPresent
                ? t("transform.apiKey.presentNote")
                : t("transform.apiKey.absentNote")}
            </p>
          </div>

          {/* Timeout (D5d) */}
          <div class="flex items-center justify-between">
            <div>
              <p class="text-sm font-medium">{t("transform.timeout.title")}</p>
              <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
                {t("transform.timeout.desc")}
              </p>
            </div>
            <input
              type="number"
              min={MIN_TRANSFORM_TIMEOUT_MS}
              max={MAX_TRANSFORM_TIMEOUT_MS}
              step={250}
              value={props.settings.timeout_ms}
              onChange={(e) => commitTimeout(e.currentTarget.value)}
              class="px-3 py-1.5 rounded-lg border text-sm w-24"
              style={inputStyle}
            />
          </div>

          {/* One-time remote-endpoint warning (D5c) */}
          <Show when={shouldWarnRemoteEndpoint(props.settings.provider.base_url, warningDismissed())}>
            <div
              role="alert"
              class="rounded-lg border p-3 text-xs flex items-start gap-2"
              style={{
                "background-color": "var(--bg)",
                "border-color": "var(--warning, #e6a700)",
              }}
            >
              <span class="leading-none mt-0.5">⚠</span>
              <div class="flex-1">
                <p class="font-medium">{t("transform.warning.title")}</p>
                <p class="mt-1" style={{ color: "var(--text-secondary)" }}>
                  {t("transform.warning.bodyIntro")}{" "}
                  <span style={{ color: "var(--text-primary)" }}>
                    {baseUrlHostname(props.settings.provider.base_url)}
                  </span>
                  {t("transform.warning.bodyOutro")}
                </p>
                <button
                  class="mt-1 underline"
                  style={{ color: "var(--accent)", cursor: "pointer" }}
                  onClick={dismissWarning}
                >
                  {t("transform.warning.dismiss")}
                </button>
              </div>
            </div>
          </Show>

          {/* Connection probe */}
          <div class="flex items-center gap-3 flex-wrap">
            <Show
              when={testState().phase === "running"}
              fallback={
                <button
                  class="px-3 py-1.5 rounded-lg text-xs font-medium border transition-colors hover:opacity-80 disabled:opacity-50"
                  style={{
                    "background-color": "var(--bg)",
                    "border-color": "var(--border)",
                    color: "var(--text-primary)",
                    cursor: testable() ? "pointer" : "not-allowed",
                  }}
                  disabled={!testable()}
                  title={testable() ? t("transform.test.titleEnabled") : t("transform.test.titleDisabled")}
                  onClick={handleTest}
                >
                  {t("transform.test.button")}
                </button>
              }
            >
              <div class="flex items-center gap-2">
                <div
                  class="w-3 h-3 rounded-full border-2 border-t-transparent animate-spin"
                  style={{ "border-color": "var(--accent)", "border-top-color": "transparent" }}
                />
                <span class="text-xs" style={{ color: "var(--text-secondary)" }}>
                  {t("transform.test.running")}
                </span>
              </div>
            </Show>
            <Show when={testOk()}>
              {(tst) => (
                <span class="text-xs font-medium" style={{ color: "var(--success)" }}>
                  {t("transform.test.ok", { ms: tst().latencyMs })}
                </span>
              )}
            </Show>
            <Show when={testError()}>
              {(tst) => (
                <span class="text-xs" style={{ color: "var(--error)" }}>
                  ✗ {tst().message}
                </span>
              )}
            </Show>
          </div>

          {/* Privacy footnote (D5) */}
          <p class="text-xs" style={{ color: "var(--text-secondary)" }}>
            {t("transform.privacyNote")}
          </p>
        </div>
      </Show>
    </div>
  );
}

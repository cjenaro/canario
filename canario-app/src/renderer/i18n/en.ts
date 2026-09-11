// English message catalog — the typed source of truth for every
// user-visible renderer string (canario-7ah.7 i18n groundwork).
//
// Shape: a FLAT record with dotted literal keys. @solid-primitives/i18n's
// `translator` types `t()` against `keyof typeof en`, so a typo'd key
// fails typecheck, and flat dotted keys keep every message one lookup
// away (no `flatten()` step needed).
//
// Values are the exact strings that previously lived inline in the
// pages/components — this round is string-for-string identical output.
// `{{ name }}` placeholders are resolved by `resolveTemplate` at call
// sites (`t("model.download", { name: "Parakeet" })`).
//
// Future locales: add a sibling file (e.g. `es.ts`) typed as
// `Partial<EnglishCatalog>` so untranslated keys can fall back to
// English (`{ ...en, ...es }` merge in index.ts), then widen `Locale`.
// Keep keys sorted by section as you add them — the completeness test
// (i18n.test.ts) fails on catalog keys no source file references.

export const en = {
  // ── Shared / cross-section ────────────────────────────────────────
  "common.rec": "REC",
  "common.loading": "Loading...",
  "common.notSet": "Not set",
  "common.cancel": "Cancel",
  "common.copy": "Copy",
  "common.copyTitle": "Copy to clipboard",
  "common.copyButton": "📋 Copy",
  "common.delete": "Delete",
  "common.browse": "Browse…",
  "common.copiedToClipboard": "Copied to clipboard",
  "common.couldNotSaveSetting": "Could not save setting.",
  "common.checking": "Checking...",
  "common.copying": "Copying…",
  "common.etaSeconds": "{{n}}s",
  "common.etaMinutes": "{{m}}m {{s}}s",

  // ── Model section (Settings + Onboarding step 1) ──────────────────
  "model.sectionTitle": "Model",
  "model.parakeetV3.name": "Parakeet TDT v3",
  "model.parakeetV3.desc": "Multilingual · ~640MB",
  "model.parakeetV2.name": "Parakeet TDT v2",
  "model.parakeetV2.desc": "English only · ~640MB",
  "model.custom.name": "Custom model",
  "model.custom.desc": "Local sherpa-onnx files · no download",
  "model.custom.hint": "Point Canario at your own sherpa-onnx files. joiner.int8.onnx must sit next to the encoder.",
  "model.custom.field.encoder": "encoder",
  "model.custom.field.decoder": "decoder",
  "model.custom.field.tokens": "tokens",
  "model.custom.missingPaths": "⚠ Set the {{fields}} path{{plural}} — recording will fail until all three are configured.",
  "model.custom.filesMissing": "⚠ One or more model files weren't found on disk — check the paths above.",
  "model.custom.ready": "✓ Custom model is ready",
  "model.download": "Download {{name}}",
  "model.downloadHint": "The ASR model runs locally on your device. Download required before first use.",
  "model.downloading": "Downloading model... this may take a few minutes.",
  "model.stopDownloadTitle": "Stop the download — progress is kept and resumed next time",
  "model.downloadCancelled": "Download cancelled — it will resume next time",
  "model.ready": "✓ {{name}} is ready",
  "model.deleteFailed": "Could not delete model.",
  "model.deleted": "Model deleted",
  "model.downloadStartFailed": "Download failed to start",
  "model.downloadCouldNotStart": "Model download could not be started",
  "model.notReady": "Model not ready",
  "model.notReadyHint.download": "Download a speech recognition model above to start transcribing.",
  "model.notReadyHint.custom":
    "Point Canario at your local model files above (encoder, decoder, tokens — plus joiner.int8.onnx next to the encoder) to start transcribing.",

  // ── Record section (Settings) ─────────────────────────────────────
  "record.sectionTitle": "Record",
  "record.recordButton": "🎤 Record",
  "record.stopTitle": "Stop and transcribe",
  "record.cancelTitle": "Cancel (discard audio, no transcription)",
  "record.cancelled": "Recording cancelled",
  "record.clickOrHotkey": "Click or press your hotkey to record",
  "record.transcribing": "Transcribing...",
  "record.listening": "Listening... speak now",
  "record.failed": "Recording failed",
  "record.noMic": "No microphone detected. Check your audio settings.",

  // ── Hotkey section (Settings) ─────────────────────────────────────
  "hotkey.sectionTitle": "Hotkey",
  "hotkey.hint": "Press-and-hold to record. Release to stop and transcribe.",
  "hotkey.doubleTapLock.title": "Double-tap to lock",
  "hotkey.doubleTapLock.desc": "Double-tap the hotkey to toggle recording on/off",
  "hotkey.minHold.title": "Minimum hold time",
  "hotkey.minHold.desc": "Seconds to hold before recording starts",
  "hotkey.doubleTapWindow.title": "Double-tap window",
  "hotkey.doubleTapWindow.desc": "Milliseconds within which two taps count as a double-tap",
  "hotkey.capture.pressKeys": "Press key combination…",
  "hotkey.capture.escHint": "(Esc to cancel)",
  "hotkey.capture.change": "Change",
  "hotkey.notice.title": "Hotkey can’t read your keyboard yet",
  "hotkey.notice.body":
    "On Linux, Canario listens for the global hotkey through /dev/input, and your user isn’t in the “input” group — so the hotkey stays silent. Recording still works from the button above, the tray, or an external trigger such as",
  "hotkey.notice.copyTitle": "Copy the command to the clipboard",
  "hotkey.notice.commandCopied": "Command copied to clipboard",
  "hotkey.notice.copyFailed": "Could not copy — select the command text and copy it manually",
  "hotkey.notice.step1": "Copy the command and run it in a terminal.",
  "hotkey.notice.step2": "Log out and back in — group membership only applies to new sessions.",
  "hotkey.notice.step3": "Start Canario again.",

  // ── Behavior section (Settings) ───────────────────────────────────
  "behavior.sectionTitle": "Behavior",
  "behavior.autoPaste.title": "Auto-paste transcription",
  "behavior.autoPaste.desc": "Automatically paste result into focused app",
  "behavior.soundEffects.title": "Sound effects",
  "behavior.soundEffects.desc": "Play sounds on recording start/stop",
  "behavior.soundVolume.title": "Sound volume",
  "behavior.soundVolume.desc": "Loudness of the beeps ({{percent}}%)",
  "behavior.liveCaptions.title": "Live captions",
  "behavior.liveCaptions.desc": "Stream a text preview in the overlay during long recordings",
  "behavior.trayIcon.title": "Show tray icon",
  "behavior.trayIcon.desc": "Show Canario in the system tray",
  "behavior.trayIcon.hidden": "Tray icon hidden — relaunch Canario to reopen this window",
  "behavior.autostart.title": "Start on login",
  "behavior.autostart.desc": "Launch Canario when you log in",
  "behavior.autostart.enabled": "Canario will start on login",
  "behavior.autostart.disabled": "Autostart disabled",
  "behavior.autostart.failed": "Could not change autostart setting.",
  "behavior.audioBehavior.title": "Audio during recording",
  "behavior.audioBehavior.desc": "System audio behavior while recording",
  "behavior.audioBehavior.doNothing": "Do nothing",
  "behavior.audioBehavior.mute": "Mute system audio",
  "behavior.audioBehavior.muteNote":
    "Mute mutes the default audio output via pactl (PulseAudio/PipeWire) while recording and restores its previous state when the recording stops or is cancelled. If pactl isn't available (e.g. macOS, Windows, or a minimal Linux install), audio simply stays on and a warning is logged.",

  // ── Microphone section (Settings) ─────────────────────────────────
  "mic.sectionTitle": "Microphone",
  "mic.title": "Dictation microphone",
  "mic.desc": "Which input device Canario records from",
  "mic.note": "Switching releases the warm microphone stream and reopens it on the new device at the next dictation.",
  "mic.systemDefault": "System default",
  "mic.notConnected": "{{name}} (not connected)",
  "mic.saveFailed": "Could not save microphone selection.",

  // ── Word Remapping section (Settings) ─────────────────────────────
  "remap.sectionTitle": "Word Remapping",
  "remap.empty.title": "No remapping rules yet. Add rules to fix common misrecognitions.",
  "remap.empty.example": "e.g. \"I llama\" → \"I'll ama\"",
  "remap.hint": "Fix common misrecognitions and remove filler words",
  "remap.tab.findReplace": "Find → Replace",
  "remap.tab.removeWords": "Remove Words",
  "remap.field.find": "Find",
  "remap.field.replace": "Replace",
  "remap.field.wordToRemove": "Word to remove",

  // ── Transformation section (Settings) ─────────────────────────────
  "transform.sectionTitle": "Transformation",
  "transform.enable.title": "Transform transcriptions",
  "transform.enable.desc": "Clean up each transcript with your own LLM before pasting (off by default)",
  "transform.baseUrl.title": "Base URL",
  "transform.baseUrl.invalid": "Enter a full http(s):// URL",
  "transform.baseUrl.placeholder": "https://api.openai.com/v1 — or http://localhost:11434/v1 (Ollama)",
  "transform.baseUrl.hint":
    "Any OpenAI-compatible endpoint, including local servers (Ollama, llama.cpp) — include the version path.",
  "transform.model.title": "Model",
  "transform.model.placeholder": "gpt-4o-mini · llama3 · qwen2.5:7b …",
  "transform.apiKey.title": "API key",
  "transform.apiKey.placeholderPresent": "API key saved — type to replace, clear + unfocus to remove",
  "transform.apiKey.placeholderAbsent": "sk-… (not needed for local endpoints)",
  "transform.apiKey.presentNote": "✓ Key saved — stored encrypted by Canario, never printed or synced",
  "transform.apiKey.absentNote": "No key stored — local endpoints (Ollama, llama.cpp server) don't need one",
  "transform.timeout.title": "Timeout",
  "transform.timeout.desc": "Milliseconds to wait before falling back to the raw transcript",
  "transform.warning.title": "Remote endpoint",
  "transform.warning.bodyIntro": "Transcripts and a short style instruction will be sent to",
  "transform.warning.bodyOutro":
    ". Nothing else ever leaves your machine — never audio, never history. Local endpoints (localhost) never leave this device.",
  "transform.warning.dismiss": "Don't show this again",
  "transform.test.button": "Test connection",
  "transform.test.running": "Testing…",
  "transform.test.ok": "✓ Connected — {{ms}} ms",
  "transform.test.titleEnabled": "Send a minimal chat-completions request",
  "transform.test.titleDisabled": "Enter a valid Base URL first",
  "transform.test.failed": "Connection test failed",
  "transform.test.unreachable": "Could not reach the speech backend",
  "transform.test.unexpected": "Unexpected response from the speech backend",
  "transform.privacyNote":
    "Your key stays on this device (encrypted at rest) and is held in the transcription backend's memory only. If the provider fails or times out, the raw transcript is pasted unchanged — dictation never blocks.",
  "transform.keySaved": "API key saved",
  "transform.keyRemoved": "API key removed",
  "transform.keySaveFailed": "Could not save the API key — try again",
  "transform.fellBack": "Transformation fell back to the raw transcript",

  // ── Appearance section (Settings) ─────────────────────────────────
  "appearance.sectionTitle": "Appearance",
  "appearance.mode.dark": "Dark",
  "appearance.mode.light": "Light",
  "appearance.mode.system": "System",
  "appearance.accent.title": "Accent color",
  "appearance.accent.desc": "Used for buttons, highlights, and the recording glow",
  "appearance.accent.defaultTitle": "Default — each theme's built-in accent",
  "appearance.accent.defaultLabel": "Default accent",
  "appearance.accent.presetAria": "{{name}} accent",
  "appearance.accent.preset.canary": "Canary",
  "appearance.accent.preset.ocean": "Ocean",
  "appearance.accent.preset.violet": "Violet",
  "appearance.accent.preset.emerald": "Emerald",
  "appearance.accent.preset.amber": "Amber",
  "appearance.accent.preset.rose": "Rose",
  "appearance.accent.custom": "Custom",
  "appearance.accent.hexPlaceholder": "#RRGGBB",
  "appearance.accent.customLabel": "Custom accent color (hex)",
  "appearance.accent.apply": "Apply",
  "appearance.accent.invalidHex": "Enter a hex color like #e94560 or #f53",

  // ── Indicator (Appearance area, Settings) ─────────────────────────
  "indicator.title": "Indicator",
  "indicator.desc": "What appears on screen while you dictate",
  "indicator.full.name": "Full overlay",
  "indicator.full.desc": "Recording pill with timer, live captions, and transcribing phases",
  "indicator.dot.name": "Dot",
  "indicator.dot.desc": "A minimal pulsing dot while recording — nothing else on screen",
  "indicator.tray.name": "Tray only",
  "indicator.tray.desc": "No on-screen indicator; the tray icon shows the recording state",
  "indicator.note":
    "The dot and the full overlay share one per-monitor position — drag the full overlay to place both. Switching modes mid-recording applies immediately; leaving “Tray only” shows the indicator again on the next recording.",
  "indicator.saveFailed": "Could not save indicator setting.",

  // ── Motion section (Settings) ─────────────────────────────────────
  "motion.sectionTitle": "Motion",
  "motion.master.title": "Animations",
  "motion.master.desc": "Play interface animations",
  "motion.reducedMotion":
    "Your system requests reduced motion — animations stay off while that OS setting is on, regardless of the toggles here.",
  "motion.effect.overlay_slide.name": "Overlay slide-in",
  "motion.effect.overlay_slide.desc": "Recording island slides down when recording starts",
  "motion.effect.recording_dot_pulse.name": "Recording dot pulse",
  "motion.effect.recording_dot_pulse.desc": "Pulsing red dot while recording",
  "motion.effect.toggle_slide.name": "Toggle slide",
  "motion.effect.toggle_slide.desc": "Switches slide and change color",
  "motion.effect.delete_slide.name": "Delete slide-out",
  "motion.effect.delete_slide.desc": "History items slide out when deleted",
  "motion.effect.window_fade.name": "Window fade-in",
  "motion.effect.window_fade.desc": "Windows fade and scale in when they open",

  // ── About section (Settings) ──────────────────────────────────────
  "about.sectionTitle": "About",
  "about.version.title": "Version",
  "about.version.withSidecar": "{{app}} (sidecar {{sidecar}})",
  "about.version.checkButton": "Check for Updates",
  "about.update.title": "Update available: v{{version}}",
  "about.update.desc": "Restart Canario to install the latest version.",
  "about.upToDate": "Canario is up to date",
  "about.updateCheckFailed": "Could not check for updates",
  "about.onboarding.title": "Onboarding",
  "about.onboarding.desc": "Replay the first-launch setup wizard",
  "about.onboarding.rerun": "Re-run",
  "about.diagnostics.title": "Diagnostics",
  "about.diagnostics.desc": "Copy system info, configuration, and recent logs",
  "about.diagnostics.copy": "Copy diagnostics",
  "about.diagnostics.collectFailed": "Could not collect diagnostics. Check that the sidecar is running.",
  "about.diagnostics.copied": "Diagnostics copied to clipboard",
  "about.diagnostics.copyFailed": "Could not copy diagnostics to the clipboard",
  "about.versionMismatch.title": "⚠ Version mismatch",
  "about.versionMismatch.noProtocol":
    "The speech backend predates protocol versioning — commands and events may have drifted. Restart with a matching build.",
  "about.versionMismatch.protocol":
    "App and speech backend speak different protocol versions (backend reports {{protocol}}). Restart with a matching build.",
  "about.versionMismatch.staleSidecar":
    "Speech backend {{sidecar}} does not match app {{app}} — a stale backend may be running. Restart Canario.",

  // ── History section (Settings) ────────────────────────────────────
  "history.sectionTitle": "History",
  "history.lastTitle": "Last Transcription",
  "history.clearAll": "Clear All",
  "history.searchPlaceholder": "🔍  Search transcriptions...",
  "history.empty.title": "No transcriptions yet",
  "history.empty.hint": "Press your hotkey and start talking!",
  "history.noResults": "No results found for “{{query}}”",
  "history.clearSearch": "Clear search",
  "history.cleared": "History cleared",
  "history.deleteFailed": "Could not delete entry.",
  "history.meta": "{{duration}}s · {{timestamp}}",
  "history.transformedBadge": "✨ Transformed",
  "history.transformedTitle": "Transformed — raw transcript: {{raw}}",
  "history.time.justNow": "Just now",
  "history.time.minutesAgo": "{{n}} min ago",
  "history.time.todayAt": "Today at {{time}}",
  "history.time.yesterdayAt": "Yesterday at {{time}}",
  "history.time.dateAt": "{{date}} at {{time}}",

  // ── Overlay window ────────────────────────────────────────────────
  "overlay.transcribing": "Transcribing…",
  "overlay.transforming": "Transforming…",
  "overlay.dragTitle": "Drag to move · double-click to reset",

  // ── Onboarding wizard ─────────────────────────────────────────────
  "onboarding.header": "Welcome to Canario",
  "onboarding.skip": "Skip setup",
  "onboarding.tagline": "Voice-to-text, instant and invisible. Press a hotkey, speak, release. Done.",
  "onboarding.step.downloadModel": "Download Model",
  "onboarding.step.setHotkey": "Set Hotkey",
  "onboarding.step.ready": "Ready",
  "onboarding.step1.title": "Step 1 of 3: Download Model",
  "onboarding.step1.desc":
    "Canario uses Parakeet TDT — a state-of-the-art speech recognition model that runs entirely on your device. Nothing you say ever leaves your machine.",
  "onboarding.step1.continueWithout":
    "You can continue without the model, but transcription won't work until it's downloaded.",
  "onboarding.step1.modelDownloaded": "Model downloaded — you're good to go!",
  "onboarding.micTest.title": "🎤 Microphone Test",
  "onboarding.micTest.start": "Test microphone",
  "onboarding.micTest.stop": "Stop",
  "onboarding.micTest.saying": "Say something...",
  "onboarding.micTest.desc": "Records {{secs}}s of audio to check your mic level.",
  "onboarding.micTest.noAccess": "Could not access the microphone. Check your audio settings.",
  "onboarding.dlStats.plain": "{{done}} / {{total}} MB",
  "onboarding.dlStats.speed": "{{done}} / {{total}} MB · {{speed}} MB/s",
  "onboarding.dlStats.etaSuffix": " · ~{{eta}} left",
  "onboarding.next": "Next →",
  "onboarding.back": "← Back",
  "onboarding.step2.title": "Step 2 of 3: Set Hotkey",
  "onboarding.step2.desc": "Pick a key combination that starts and stops recording from anywhere.",
  "onboarding.step2.pressHoldLabel": "Press-and-hold:",
  "onboarding.step2.pressHoldBody": "hold the combo while you speak, release to transcribe.",
  "onboarding.step2.doubleTapLabel": "Double-tap:",
  "onboarding.step2.doubleTapBody": "tap the combo to start recording, tap again to stop — hands-free for longer dictation.",
  "onboarding.step2.linux": "On Linux the hotkey is handled by Canario's own listener.",
  "onboarding.step2.other": "On this platform the hotkey is registered globally with the OS.",
  "onboarding.step3.title": "Step 3 of 3: Ready",
  "onboarding.step3.descIntro": "Try it now! Click the field below, press",
  "onboarding.step3.yourHotkey": "your hotkey",
  "onboarding.step3.descOutro": ", speak, and release — your words will appear right here.",
  "onboarding.step3.placeholder": "Press your hotkey and say something…",
  "onboarding.step3.works": "✓ It works! Last transcription: \"{{text}}\"",
  "onboarding.step3.noModel":
    "Heads up: no speech model is downloaded yet, so practice dictation won't transcribe. You can download it later from Settings → Model.",
  "onboarding.done": "Done — minimize to tray",
  "onboarding.initFailed": "Failed to initialize. Check that the canario sidecar is running.",

  // ── Renderer-authored error strings (createCanario bridge) ────────
  // Sidecar/main-authored error text (sidecar `error` fields, wire
  // `message`s) passes through untranslated — see i18n/index.ts notes.
  "errors.sidecarCrashed": "Speech backend exited unexpectedly (code {{code}}) — please restart Canario",
  "errors.initFailed.electron": "Failed to initialize. Check that canario-electron sidecar is running.",
} as const;

/** The English catalog — also the type every future locale is checked against. */
export type EnglishCatalog = typeof en;

/** Every valid `t()` key. A typo'd key is a typecheck error. */
export type MessageKey = keyof EnglishCatalog;

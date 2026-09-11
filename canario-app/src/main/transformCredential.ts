// Transform provider credential vault (canario-fgm.2, decision D2).
//
// The provider API key is the only secret Canario ever holds. It is
// NEVER written to config.json (AppConfig.transform carries provider
// metadata only) and NEVER returned to the renderer — the settings
// key field is write-only and `transform_status` exposes only
// `credential_present`. Persistence lives HERE, main-process only:
//
//   userData/transform-key.bin   safeStorage blob, perms 0600
//
// The sidecar receives a copy at boot (initTransformCredential, called
// from index.ts once the sidecar answers ping) and on every change
// (saveTransformCredential, the "transform:setKey" IPC handler) via
// the `set_transform_credential` command, which keeps it in MEMORY
// ONLY — killing the sidecar wipes its copy.
//
// Linux choice (documented per the fgm.2 scope): without a keyring
// (gnome-keyring/kwallet) safeStorage refuses to encrypt, so
// `ensureLinuxPlaintextFallback` opts into Electron's basic_text
// fallback. That blob is only obfuscated (hardcoded-password v10
// format), NOT cryptographically protected at rest — the same trade
// Chromium makes for its own secrets on such systems; the 0600 file
// perms are the remaining guard. `safeStorage
// .getSelectedStorageBackend()` reports "basic_text" in that state
// ("gnome_libsecret"/"kwallet"/… otherwise). The flag is a no-op when
// a keyring exists and on Windows/macOS entirely.

import { app, safeStorage } from "electron";
import { chmodSync, existsSync, readFileSync, unlinkSync, writeFileSync } from "fs";
import { join } from "path";
import { sendCommand } from "./sidecar.js";

// Ids only need uniqueness while a response is pending (the sidecar
// matches responses by id) — a module counter suffices, same pattern
// as autostart.ts.
let credentialCommandSeq = 0;

function nextCredentialId(): string {
  credentialCommandSeq += 1;
  return `transform-credential-${credentialCommandSeq}`;
}

function credentialFilePath(): string {
  return join(app.getPath("userData"), "transform-key.bin");
}

/** See the module doc: enable basic_text BEFORE the first availability
 *  check so encrypt/decrypt work on keyring-less Linux installs. */
function ensureLinuxPlaintextFallback(): void {
  if (process.platform !== "linux") return;
  try {
    safeStorage.setUsePlainTextEncryption(true);
  } catch (err) {
    console.warn("[transform] Could not enable the safeStorage plaintext fallback:", err);
  }
}

/**
 * Read + decrypt the stored key. null when absent, unreadable, or
 * undecryptable (e.g. the blob was written under another keyring or
 * install) — callers treat that as "no key": the user re-enters it and
 * the next save overwrites the dead file.
 */
export function loadTransformCredential(): string | null {
  ensureLinuxPlaintextFallback();
  let blob: Buffer;
  try {
    blob = readFileSync(credentialFilePath());
  } catch {
    return null; // no stored key — the common case
  }
  if (!safeStorage.isEncryptionAvailable()) {
    console.warn("[transform] safeStorage unavailable — ignoring the stored API key");
    return null;
  }
  try {
    const key = safeStorage.decryptString(blob);
    return key.trim().length > 0 ? key : null;
  } catch (err) {
    console.warn("[transform] Stored API key could not be decrypted:", err);
    return null;
  }
}

export interface TransformCredentialResult {
  /** False when persistence or the sidecar push failed. */
  ok: boolean;
  /** True when the sidecar now holds a key — false after a clear. */
  stored: boolean;
  /** Failure detail for ok:false (safe to surface in a toast). */
  error?: string;
}

/** Deliver a key (or null) to the sidecar's memory. */
async function pushCredential(key: string | null): Promise<TransformCredentialResult> {
  try {
    const res = await sendCommand({
      id: nextCredentialId(),
      cmd: "set_transform_credential",
      key,
    });
    if (res.ok !== true) {
      return {
        ok: false,
        stored: false,
        error: `Backend rejected the credential update: ${String(res.error ?? "unknown error")}`,
      };
    }
    const stored = (res.data as { stored?: unknown } | undefined)?.stored === true;
    return { ok: true, stored };
  } catch (err) {
    return {
      ok: false,
      stored: false,
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

/**
 * Boot path (fgm.2 second half): load the stored key and push it into
 * the sidecar's memory. Best effort by design — a missing key just
 * leaves cloud providers without a credential (local endpoints need
 * none) and the settings section re-pushes on every change.
 */
export async function initTransformCredential(): Promise<void> {
  const key = loadTransformCredential();
  const result = await pushCredential(key);
  if (!result.ok) {
    console.warn("[transform] Startup credential push failed:", result.error);
    return;
  }
  if (result.stored) {
    console.log("[transform] Provider API key loaded into the sidecar");
  }
}

/**
 * Store a new key (empty/whitespace CLEARS — see clearTransformCredential)
 * and push it to the sidecar. Persists to disk BEFORE pushing: if the
 * push fails the key still survives and the next boot's
 * initTransformCredential delivers it.
 */
export async function saveTransformCredential(key: string): Promise<TransformCredentialResult> {
  ensureLinuxPlaintextFallback();
  const trimmed = key.trim();
  if (trimmed.length === 0) {
    return clearTransformCredential();
  }
  if (!safeStorage.isEncryptionAvailable()) {
    return {
      ok: false,
      stored: false,
      error: "System key storage is unavailable — could not save the API key",
    };
  }
  let blob: Buffer;
  try {
    blob = safeStorage.encryptString(trimmed);
  } catch (err) {
    return {
      ok: false,
      stored: false,
      error: `Could not encrypt the API key: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
  try {
    const path = credentialFilePath();
    // writeFileSync's mode only applies at file creation — enforce the
    // 0600 perms on pre-existing files too (best effort).
    writeFileSync(path, blob, { mode: 0o600 });
    try {
      chmodSync(path, 0o600);
    } catch {
      /* perms are best-effort hardening, not a gate */
    }
  } catch (err) {
    return {
      ok: false,
      stored: false,
      error: `Could not write the key file: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
  return pushCredential(trimmed);
}

/** The clear path (the user emptied the key field): drop the file AND
 *  the sidecar's in-memory copy. Idempotent. */
export async function clearTransformCredential(): Promise<TransformCredentialResult> {
  try {
    const path = credentialFilePath();
    if (existsSync(path)) {
      unlinkSync(path);
    }
  } catch (err) {
    return {
      ok: false,
      stored: false,
      error: `Could not remove the key file: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
  return pushCredential(null);
}

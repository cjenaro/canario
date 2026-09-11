// Onboarding flag helpers (canario-xv9).
//
// The completion flag lives in the sidecar-owned AppConfig
// (`onboarding_completed`), read/written by index.ts through the
// sidecar's get_config / update_config commands. This module holds the
// pure pieces so they stay unit-testable without Electron.

/**
 * Parse the contents of the legacy main-process onboarding.json
 * (`{"completed": true}` — written by the Electron main process before
 * canario-xv9, mirroring theme.json).
 *
 * Returns true only when the file's `completed` value coerces truthy —
 * the exact semantics of the pre-migration read (`!!...completed`), so
 * the import preserves the user's observable state. `null` (file
 * absent — the common case after migration) and corrupt/empty contents
 * count as not completed, so the one-time import never weakens the
 * AppConfig value.
 */
export function parseLegacyOnboardingFile(raw: string | null): boolean {
  if (raw === null) return false;
  try {
    return !!JSON.parse(raw).completed;
  } catch {
    return false;
  }
}

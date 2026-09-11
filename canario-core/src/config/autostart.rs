/// Autostart and .desktop file management.
///
/// On first launch (or when requested), installs the .desktop file
/// to `~/.local/share/applications/` and optionally creates an entry
/// in `~/.config/autostart/` to start Canario on login.
///
/// There is exactly ONE login-entry identity —
/// `com.canario.Canario.desktop` — shared by every frontend
/// (canario-dmp.17). Frontends that need their own launch command (the
/// Electron app, which must launch the Electron binary rather than the
/// menu entry's `Exec=canario`) pass an explicit `exec` to
/// [`enable_autostart`] and get a standalone regular file; everything
/// else symlinks the installed menu entry. The legacy Electron-written
/// `canario.desktop` is absorbed at startup by
/// [`migrate_legacy_autostart`] so enabling from both frontends can
/// never produce two login entries (and two contending hotkey
/// backends).
///
/// This follows freedesktop conventions and is only implemented on
/// Linux. On Windows and macOS every fallible operation returns an
/// explicit "not supported" error — callers must never see a false
/// success for a no-op.
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use tracing::{info, warn};

/// Get the autostart directory (`~/.config/autostart/`).
#[cfg(target_os = "linux")]
fn autostart_dir() -> PathBuf {
    dirs::config_dir()
        .expect("No config directory")
        .join("autostart")
}

/// Get the applications directory (`~/.local/share/applications/`).
#[cfg(target_os = "linux")]
fn applications_dir() -> PathBuf {
    dirs::data_dir()
        .expect("No data directory")
        .join("applications")
}

/// The .desktop file name — the menu entry AND the single login-entry
/// identity shared by all frontends.
#[cfg(any(target_os = "linux", test))]
const DESKTOP_FILE: &str = "com.canario.Canario.desktop";

/// The legacy login entry the Electron main process used to write
/// itself (canario-dmp.17). Referenced only to migrate or remove it —
/// nothing may create it anymore.
#[cfg(any(target_os = "linux", test))]
const LEGACY_DESKTOP_FILE: &str = "canario.desktop";

/// The contents of the .desktop file.
#[cfg(any(target_os = "linux", test))]
const DESKTOP_CONTENTS: &str = "\
[Desktop Entry]
Type=Application
Name=Canario
GenericName=Voice to Text
Comment=Native Linux voice-to-text using Parakeet TDT
Exec=canario
Icon=com.canario.Canario
Terminal=false
Categories=Utility;AudioVideo;
Keywords=voice;speech;text;transcription;dictation;
StartupNotify=false
";

/// Build the contents of the standalone login entry for a
/// frontend-specific `exec` — mirrors the entry the Electron main
/// process used to write itself, so login behavior is unchanged for
/// Electron users.
#[cfg(any(target_os = "linux", test))]
fn autostart_contents(exec: &str) -> String {
    format!(
        "\
[Desktop Entry]
Type=Application
Name=Canario
Comment=Voice-to-text
Exec={exec}
Icon=com.canario.Canario
Terminal=false
Categories=Utility;
X-GNOME-Autostart-enabled=true
Hidden=false
"
    )
}

/// Explicit error for platforms where freedesktop autostart /
/// .desktop-file integration does not apply (Windows, macOS).
///
/// Keeping this a single helper guarantees every stub reports the same
/// unambiguous message instead of silently pretending success.
#[cfg(not(target_os = "linux"))]
fn unsupported<T>(operation: &str) -> anyhow::Result<T> {
    anyhow::bail!(
        "{operation} is not supported on this platform \
         (autostart/desktop integration is currently Linux-only)"
    )
}

/// Install the .desktop file to `~/.local/share/applications/` if not already there.
///
/// This makes Canario appear in application menus. Called once on first launch.
#[cfg(target_os = "linux")]
pub fn install_desktop_file() -> anyhow::Result<()> {
    let dir = applications_dir();
    std::fs::create_dir_all(&dir)?;

    let dest = dir.join(DESKTOP_FILE);

    if dest.exists() {
        // Check if the content matches — update if stale
        let existing = std::fs::read_to_string(&dest).unwrap_or_default();
        if existing.trim() == DESKTOP_CONTENTS.trim() {
            return Ok(());
        }
        info!("Updating .desktop file at {:?}", dest);
    } else {
        info!("Installing .desktop file to {:?}", dest);
    }

    std::fs::write(&dest, DESKTOP_CONTENTS)?;
    Ok(())
}

/// Install the .desktop file to the applications directory.
///
/// Not supported off Linux — returns an explicit error.
#[cfg(not(target_os = "linux"))]
pub fn install_desktop_file() -> anyhow::Result<()> {
    unsupported("install_desktop_file")
}

/// Remove the legacy Electron-written login entry, if present.
///
/// Shared by [`enable_autostart`] (absorb before rewriting),
/// [`disable_autostart`], and [`migrate_legacy_autostart`]'s
/// both-exist case. Uses `symlink_metadata` so a dangling legacy
/// symlink is caught too.
#[cfg(target_os = "linux")]
fn remove_legacy_entry() -> anyhow::Result<()> {
    let legacy = autostart_dir().join(LEGACY_DESKTOP_FILE);
    if legacy.symlink_metadata().is_ok() {
        std::fs::remove_file(&legacy)?;
        info!("Removed legacy autostart entry: {:?}", legacy);
    }
    Ok(())
}

/// Enable autostart — create an entry in `~/.config/autostart/`.
///
/// `exec = None` (Canario's own launchers): make sure the menu
/// .desktop file is installed, then symlink it into the autostart
/// directory (falling back to a copy when symlinks are unavailable).
/// `exec = Some(cmd)` (foreign frontends like the Electron app): write
/// a standalone regular entry whose `Exec` is `cmd`.
///
/// The legacy Electron entry is removed first and the new-identity
/// entry is always remove+rewritten — re-enabling with a different
/// `exec` must replace the old entry, and an identical rewrite is
/// idempotent, so there is never more than one login entry.
#[cfg(target_os = "linux")]
pub fn enable_autostart(exec: Option<&str>) -> anyhow::Result<()> {
    let autostart = autostart_dir().join(DESKTOP_FILE);

    // Absorb the legacy Electron entry, then clear the new-identity
    // slot: it may be a symlink, a regular file with a different Exec,
    // or even a dangling symlink (which `Path::exists` would miss).
    remove_legacy_entry()?;
    std::fs::create_dir_all(autostart.parent().unwrap())?;
    if autostart.symlink_metadata().is_ok() {
        std::fs::remove_file(&autostart)?;
    }

    match exec {
        Some(exec) => {
            std::fs::write(&autostart, autostart_contents(exec))?;
        }
        None => {
            // Make sure the source .desktop file exists first
            install_desktop_file()?;

            // Try symlink first (preferred), fall back to copy
            let applications = applications_dir().join(DESKTOP_FILE);
            if let Err(e) = std::os::unix::fs::symlink(&applications, &autostart) {
                warn!("Symlink failed ({}), copying instead", e);
                std::fs::copy(&applications, &autostart)?;
            }
        }
    }

    info!("Autostart enabled: {:?}", autostart);
    Ok(())
}

/// Enable autostart.
///
/// Not supported off Linux — returns an explicit error rather than
/// pretending the login entry was created.
#[cfg(not(target_os = "linux"))]
pub fn enable_autostart(_exec: Option<&str>) -> anyhow::Result<()> {
    unsupported("enable_autostart")
}

/// Disable autostart — remove the login entry from `~/.config/autostart/`.
///
/// Removes both the new-identity entry and the legacy Electron-written
/// one, so a pre-migration install ends up with zero login entries.
#[cfg(target_os = "linux")]
pub fn disable_autostart() -> anyhow::Result<()> {
    let autostart = autostart_dir().join(DESKTOP_FILE);

    if autostart.symlink_metadata().is_ok() {
        std::fs::remove_file(&autostart)?;
        info!("Autostart disabled");
    }
    remove_legacy_entry()?;

    Ok(())
}

/// Disable autostart.
///
/// Not supported off Linux — returns an explicit error.
#[cfg(not(target_os = "linux"))]
pub fn disable_autostart() -> anyhow::Result<()> {
    unsupported("disable_autostart")
}

/// Check if autostart is currently enabled.
#[cfg(target_os = "linux")]
pub fn is_autostart_enabled() -> anyhow::Result<bool> {
    let autostart = autostart_dir().join(DESKTOP_FILE);
    Ok(autostart.exists())
}

/// Check if autostart is currently enabled.
///
/// Not supported off Linux — returns an explicit error instead of a
/// misleading `false` (which frontends could read as "disabled").
#[cfg(not(target_os = "linux"))]
pub fn is_autostart_enabled() -> anyhow::Result<bool> {
    unsupported("is_autostart_enabled")
}

/// Absorb the legacy Electron-written login entry
/// (`~/.config/autostart/canario.desktop`) into the unified identity
/// (`com.canario.Canario.desktop`).
///
/// - legacy present, new identity absent → renamed, preserving the
///   user's enabled state and their Electron `Exec`;
/// - both present → legacy removed (the new identity wins) — the exact
///   double-entry split-brain canario-dmp.17 fixes;
/// - neither → no-op.
///
/// Called at startup by every frontend so the migration happens even
/// if the user never opens settings again.
#[cfg(target_os = "linux")]
pub fn migrate_legacy_autostart() -> anyhow::Result<()> {
    let dir = autostart_dir();
    let legacy = dir.join(LEGACY_DESKTOP_FILE);
    let current = dir.join(DESKTOP_FILE);

    if legacy.symlink_metadata().is_err() {
        return Ok(());
    }

    if current.symlink_metadata().is_ok() {
        std::fs::remove_file(&legacy)?;
        info!(
            "Removed legacy autostart entry {:?} — {} already exists",
            legacy, DESKTOP_FILE
        );
    } else {
        std::fs::rename(&legacy, &current)?;
        info!(
            "Renamed legacy autostart entry {:?} → {:?} \
             (enabled state and Exec preserved)",
            legacy, current
        );
    }
    Ok(())
}

/// Nothing to migrate on other platforms — the legacy entry is a
/// Linux freedesktop concept that never existed elsewhere, so this is
/// an unconditional success.
#[cfg(not(target_os = "linux"))]
pub fn migrate_legacy_autostart() -> anyhow::Result<()> {
    Ok(())
}

/// Get the path where the icon should be installed for the .desktop file to find it.
pub fn icon_install_path() -> PathBuf {
    dirs::data_dir()
        .expect("No data directory")
        .join("icons")
        .join("hicolor")
        .join("scalable")
        .join("apps")
        .join("com.canario.Canario.svg")
}

/// Install the SVG icon to the system icon path.
#[cfg(target_os = "linux")]
pub fn install_icon(icon_svg: &[u8]) -> anyhow::Result<()> {
    let dest = icon_install_path();
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(&dest, icon_svg)?;
    info!("Icon installed to {:?}", dest);
    Ok(())
}

/// Install the SVG icon to the system icon path.
///
/// Not supported off Linux — returns an explicit error.
#[cfg(not(target_os = "linux"))]
pub fn install_icon(_icon_svg: &[u8]) -> anyhow::Result<()> {
    unsupported("install_icon")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_contents_valid() {
        // Basic sanity: should start with [Desktop Entry]
        assert!(DESKTOP_CONTENTS.starts_with("[Desktop Entry]"));
        assert!(DESKTOP_CONTENTS.contains("Exec=canario"));
        assert!(DESKTOP_CONTENTS.contains("Icon=com.canario.Canario"));
    }

    /// The .desktop file name must be a well-formed freedesktop ID and
    /// must match the icon referenced inside the contents, otherwise
    /// menus won't associate the two.
    #[test]
    fn desktop_file_name_matches_icon_reference() {
        assert!(DESKTOP_FILE.ends_with(".desktop"));
        let stem = DESKTOP_FILE.trim_end_matches(".desktop");
        assert_eq!(stem, "com.canario.Canario");
        assert!(DESKTOP_CONTENTS.contains(&format!("Icon={stem}")));
    }

    /// Every non-header, non-empty line must be a `Key=Value` pair —
    /// the file is written verbatim to disk, so a malformed line would
    /// make the desktop entry silently disappear from menus.
    #[test]
    fn desktop_contents_lines_are_key_value_pairs() {
        for (i, line) in DESKTOP_CONTENTS.lines().enumerate() {
            if i == 0 {
                assert_eq!(line, "[Desktop Entry]");
                continue;
            }
            if line.is_empty() {
                continue;
            }
            assert!(
                line.contains('='),
                "line {i} is not a Key=Value pair: {line:?}"
            );
            assert_eq!(line, line.trim(), "line {i} has stray whitespace: {line:?}");
        }
    }

    /// The required keys per the desktop entry spec must all be present.
    #[test]
    fn desktop_contents_has_required_keys() {
        for key in ["Type=", "Name=", "Exec=", "Categories=", "StartupNotify="] {
            assert!(DESKTOP_CONTENTS.contains(key), "missing required key {key}");
        }
        assert!(DESKTOP_CONTENTS.contains("Type=Application"));
    }

    /// The standalone login entry embeds the caller's exec verbatim
    /// and carries the autostart-relevant keys the Electron writer
    /// used to emit.
    #[test]
    fn autostart_contents_embed_exec_and_autostart_keys() {
        let contents = autostart_contents("/usr/bin/fake-canario");
        assert!(contents.starts_with("[Desktop Entry]"));
        assert!(contents.contains("Exec=/usr/bin/fake-canario"));
        assert!(contents.contains("Name=Canario"));
        assert!(contents.contains("Comment=Voice-to-text"));
        assert!(contents.contains("Icon=com.canario.Canario"));
        assert!(contents.contains("Terminal=false"));
        assert!(contents.contains("Categories=Utility;"));
        assert!(contents.contains("X-GNOME-Autostart-enabled=true"));
        assert!(contents.contains("Hidden=false"));
    }

    /// A different exec must produce different contents — this is what
    /// makes the always-remove+rewrite re-enable in
    /// `enable_autostart` correct.
    #[test]
    fn autostart_contents_differs_per_exec() {
        assert_ne!(
            autostart_contents("/usr/bin/fake-canario"),
            autostart_contents("/opt/canario/other")
        );
    }

    /// Same Key=Value hygiene as the menu entry — the standalone
    /// entry is written verbatim to disk.
    #[test]
    fn autostart_contents_lines_are_key_value_pairs() {
        let contents = autostart_contents("/usr/bin/fake-canario");
        for (i, line) in contents.lines().enumerate() {
            if i == 0 {
                assert_eq!(line, "[Desktop Entry]");
                continue;
            }
            if line.is_empty() {
                continue;
            }
            assert!(
                line.contains('='),
                "line {i} is not a Key=Value pair: {line:?}"
            );
            assert_eq!(line, line.trim(), "line {i} has stray whitespace: {line:?}");
        }
    }

    /// The standalone entry needs the desktop-entry-spec required keys
    /// (no StartupNotify: the Electron entry it mirrors didn't set it).
    #[test]
    fn autostart_contents_has_required_keys() {
        let contents = autostart_contents("/usr/bin/fake-canario");
        for key in ["Type=", "Name=", "Exec=", "Categories="] {
            assert!(contents.contains(key), "missing required key {key}");
        }
        assert!(contents.contains("Type=Application"));
    }

    /// The legacy name must be exactly the file the old Electron
    /// writer created, and must differ from the unified identity it is
    /// migrated into.
    #[test]
    fn legacy_entry_name_matches_old_electron_writer() {
        assert_eq!(LEGACY_DESKTOP_FILE, "canario.desktop");
        assert_ne!(LEGACY_DESKTOP_FILE, DESKTOP_FILE);
    }

    /// On unsupported platforms every fallible operation must fail
    /// loudly — never report success for a no-op. (On Linux this test
    /// is vacuous; the real behavior is exercised by the frontends.)
    /// Migration is the one deliberate exception: there is no legacy
    /// entry to absorb on Windows/macOS, so it succeeds as a no-op.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_platforms_return_explicit_errors() {
        for result in [
            install_desktop_file(),
            enable_autostart(None),
            disable_autostart(),
            install_icon(b"<svg/>"),
        ] {
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("not supported on this platform"),
                "unexpected error message: {err}"
            );
        }
        assert!(is_autostart_enabled()
            .unwrap_err()
            .to_string()
            .contains("not supported on this platform"));
        migrate_legacy_autostart().unwrap();
    }
}

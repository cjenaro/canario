/// Autostart and .desktop file management.
///
/// On first launch (or when requested), installs the .desktop file
/// to `~/.local/share/applications/` and optionally creates a symlink
/// in `~/.config/autostart/` to start Canario on login.
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

/// The .desktop file name.
#[cfg(any(target_os = "linux", test))]
const DESKTOP_FILE: &str = "com.canario.Canario.desktop";

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

/// Enable autostart — create a symlink in `~/.config/autostart/`.
#[cfg(target_os = "linux")]
pub fn enable_autostart() -> anyhow::Result<()> {
    let autostart = autostart_dir().join(DESKTOP_FILE);
    let applications = applications_dir().join(DESKTOP_FILE);

    // Make sure the source .desktop file exists first
    install_desktop_file()?;

    std::fs::create_dir_all(autostart.parent().unwrap())?;

    // Remove old entry if it exists (could be symlink or regular file)
    if autostart.exists() {
        if is_autostart_enabled()? {
            return Ok(()); // Already enabled
        }
        std::fs::remove_file(&autostart)?;
    }

    // Try symlink first (preferred), fall back to copy
    if let Err(e) = std::os::unix::fs::symlink(&applications, &autostart) {
        warn!("Symlink failed ({}), copying instead", e);
        std::fs::copy(&applications, &autostart)?;
    }

    info!("Autostart enabled: {:?}", autostart);
    Ok(())
}

/// Enable autostart.
///
/// Not supported off Linux — returns an explicit error rather than
/// pretending the login entry was created.
#[cfg(not(target_os = "linux"))]
pub fn enable_autostart() -> anyhow::Result<()> {
    unsupported("enable_autostart")
}

/// Disable autostart — remove the symlink/file from `~/.config/autostart/`.
#[cfg(target_os = "linux")]
pub fn disable_autostart() -> anyhow::Result<()> {
    let autostart = autostart_dir().join(DESKTOP_FILE);

    if autostart.exists() {
        std::fs::remove_file(&autostart)?;
        info!("Autostart disabled");
    }

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

    /// On unsupported platforms every fallible operation must fail
    /// loudly — never report success for a no-op. (On Linux this test
    /// is vacuous; the real behavior is exercised by the frontends.)
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_platforms_return_explicit_errors() {
        for result in [
            install_desktop_file(),
            enable_autostart(),
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
    }
}

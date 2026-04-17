/// Autostart and .desktop file management.
///
/// On first launch (or when requested), installs the .desktop file
/// to `~/.local/share/applications/` and optionally creates a symlink
/// in `~/.config/autostart/` to start Canario on login.

use std::path::PathBuf;
use tracing::{info, warn};

/// Get the autostart directory (`~/.config/autostart/`).
fn autostart_dir() -> PathBuf {
    dirs::config_dir()
        .expect("No config directory")
        .join("autostart")
}

/// Get the applications directory (`~/.local/share/applications/`).
fn applications_dir() -> PathBuf {
    dirs::data_dir()
        .expect("No data directory")
        .join("applications")
}

/// The .desktop file name.
const DESKTOP_FILE: &str = "com.canario.Canario.desktop";

/// The contents of the .desktop file.
const DESKTOP_CONTENTS: &str = "\
[Desktop Entry]
Type=Application
Name=Canario
GenericName=Voice to Text
Comment=Native Linux voice-to-text using Parakeet TDT
Exec=canario
Icon=canario
Terminal=false
Categories=Utility;AudioVideo;
Keywords=voice;speech;text;transcription;dictation;
StartupNotify=false
";

/// Install the .desktop file to `~/.local/share/applications/` if not already there.
///
/// This makes Canario appear in application menus. Called once on first launch.
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

/// Enable autostart — create a symlink in `~/.config/autostart/`.
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

/// Disable autostart — remove the symlink/file from `~/.config/autostart/`.
pub fn disable_autostart() -> anyhow::Result<()> {
    let autostart = autostart_dir().join(DESKTOP_FILE);

    if autostart.exists() {
        std::fs::remove_file(&autostart)?;
        info!("Autostart disabled");
    }

    Ok(())
}

/// Check if autostart is currently enabled.
pub fn is_autostart_enabled() -> anyhow::Result<bool> {
    let autostart = autostart_dir().join(DESKTOP_FILE);
    Ok(autostart.exists())
}

/// Get the path where the icon should be installed for the .desktop file to find it.
pub fn icon_install_path() -> PathBuf {
    dirs::data_dir()
        .expect("No data directory")
        .join("icons")
        .join("hicolor")
        .join("scalable")
        .join("apps")
        .join("canario.svg")
}

/// Install the SVG icon to the system icon path.
pub fn install_icon(icon_svg: &[u8]) -> anyhow::Result<()> {
    let dest = icon_install_path();
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(&dest, icon_svg)?;
    info!("Icon installed to {:?}", dest);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_contents_valid() {
        // Basic sanity: should start with [Desktop Entry]
        assert!(DESKTOP_CONTENTS.starts_with("[Desktop Entry]"));
        assert!(DESKTOP_CONTENTS.contains("Exec=canario"));
        assert!(DESKTOP_CONTENTS.contains("Icon=canario"));
    }
}

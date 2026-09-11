//! Integration tests for autostart (login entry) management.
//!
//! The functions under test derive their paths from `dirs::config_dir()`
//! and `dirs::data_dir()`, which honor `$XDG_CONFIG_HOME` /
//! `$XDG_DATA_HOME` (read at call time, not cached). Tests redirect both
//! to per-test temp dirs; a process-wide mutex serializes them because
//! env vars are shared across threads in this test binary (same pattern
//! as tests/config_integration.rs).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use canario_core::autostart::{
    disable_autostart, enable_autostart, is_autostart_enabled, migrate_legacy_autostart,
};

/// The one login-entry identity shared by all frontends.
const DESKTOP_FILE: &str = "com.canario.Canario.desktop";
/// The legacy entry the Electron main process used to write.
const LEGACY_DESKTOP_FILE: &str = "canario.desktop";

/// Serializes tests that mutate process env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    // Keep the temp dirs alive for the duration of the test.
    _config: tempfile::TempDir,
    _data: tempfile::TempDir,
}

/// Point XDG_CONFIG_HOME / XDG_DATA_HOME at fresh temp dirs and return
/// the (autostart, applications) directories they imply.
fn isolated_dirs() -> (EnvGuard, PathBuf, PathBuf) {
    let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let autostart_dir = config.path().join("autostart");
    let applications_dir = data.path().join("applications");
    std::env::set_var("XDG_CONFIG_HOME", config.path());
    std::env::set_var("XDG_DATA_HOME", data.path());
    (
        EnvGuard {
            _lock: lock,
            _config: config,
            _data: data,
        },
        autostart_dir,
        applications_dir,
    )
}

/// A plausible pre-unification Electron login entry (the contents the
/// deleted canario-app writer used to emit).
fn legacy_contents() -> String {
    "\
[Desktop Entry]
Type=Application
Name=Canario
Comment=Voice-to-text
Exec=/opt/Canario/canario-electron
Icon=com.canario.Canario
Terminal=false
Categories=Utility;
X-GNOME-Autostart-enabled=true
Hidden=false
"
    .to_string()
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Count entries in a directory (0 when it doesn't exist).
fn entry_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}

// ── enable with exec (standalone entry) ──────────────────────────────────

#[test]
fn set_autostart_with_exec_writes_regular_file_and_persists_flag() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    let (canario, _rx) = canario_core::Canario::new().unwrap();
    assert!(!canario.config().autostart);

    canario
        .set_autostart(true, Some("/usr/bin/fake-canario"))
        .unwrap();

    let entry = autostart_dir.join(DESKTOP_FILE);
    assert!(
        !is_symlink(&entry),
        "exec enable must write a regular file, not a symlink"
    );
    let contents = std::fs::read_to_string(&entry).unwrap();
    assert!(
        contents.contains("Exec=/usr/bin/fake-canario"),
        "entry should embed the given exec: {contents}"
    );
    assert!(contents.contains("X-GNOME-Autostart-enabled=true"));
    assert!(is_autostart_enabled().unwrap());

    // The flag agrees with disk — it is the single source of truth.
    assert!(canario.config().autostart);

    // Re-enabling with a different exec REWRITES the entry instead of
    // leaving a stale one (the split-brain canario-dmp.17 fix).
    canario
        .set_autostart(true, Some("/opt/canario/other"))
        .unwrap();
    let contents = std::fs::read_to_string(&entry).unwrap();
    assert!(contents.contains("Exec=/opt/canario/other"), "{contents}");
    assert!(!contents.contains("fake-canario"), "{contents}");
    assert_eq!(
        entry_count(&autostart_dir),
        1,
        "exactly one login entry may exist"
    );
}

#[test]
fn failed_enable_leaves_flag_untouched() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    let (canario, _rx) = canario_core::Canario::new().unwrap();
    assert!(!canario.config().autostart);

    // Sabotage the config root AFTER Canario::new(): pointing
    // XDG_CONFIG_HOME at a regular file makes the autostart dir
    // impossible to create.
    let blocker = autostart_dir.parent().unwrap().join("not-a-dir");
    std::fs::write(&blocker, b"").unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &blocker);

    assert!(
        canario
            .set_autostart(true, Some("/usr/bin/fake-canario"))
            .is_err(),
        "enable must fail when the autostart dir cannot be created"
    );
    assert!(
        !canario.config().autostart,
        "flag must stay untouched when the fs op fails"
    );
}

// ── enable without exec (symlink to the menu entry) ──────────────────────

#[test]
fn set_autostart_without_exec_symlinks_the_menu_entry() {
    let (_guard, autostart_dir, applications_dir) = isolated_dirs();
    let (canario, _rx) = canario_core::Canario::new().unwrap();

    canario.set_autostart(true, None).unwrap();

    let entry = autostart_dir.join(DESKTOP_FILE);
    assert!(
        is_symlink(&entry),
        "None enable must symlink the menu entry"
    );
    assert_eq!(
        std::fs::read_link(&entry).unwrap(),
        applications_dir.join(DESKTOP_FILE)
    );
    // The symlink target was installed and runs the menu Exec.
    let menu = std::fs::read_to_string(applications_dir.join(DESKTOP_FILE)).unwrap();
    assert!(menu.contains("Exec=canario"), "{menu}");
    assert!(canario.config().autostart);
}

#[test]
fn enable_without_exec_absorbs_legacy_entry() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    std::fs::create_dir_all(&autostart_dir).unwrap();
    std::fs::write(autostart_dir.join(LEGACY_DESKTOP_FILE), legacy_contents()).unwrap();

    enable_autostart(None).unwrap();

    assert!(
        !autostart_dir.join(LEGACY_DESKTOP_FILE).exists(),
        "enabling must absorb the legacy entry"
    );
    assert!(is_symlink(&autostart_dir.join(DESKTOP_FILE)));
    assert_eq!(entry_count(&autostart_dir), 1);
}

// ── disable ───────────────────────────────────────────────────────────────

#[test]
fn set_autostart_disable_removes_entry_and_flag() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    let (canario, _rx) = canario_core::Canario::new().unwrap();
    canario
        .set_autostart(true, Some("/usr/bin/fake-canario"))
        .unwrap();

    canario.set_autostart(false, None).unwrap();

    assert!(!autostart_dir.join(DESKTOP_FILE).exists());
    assert!(!is_autostart_enabled().unwrap());
    assert!(!canario.config().autostart);
}

#[test]
fn disable_also_removes_legacy_entry() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    std::fs::create_dir_all(&autostart_dir).unwrap();
    std::fs::write(autostart_dir.join(LEGACY_DESKTOP_FILE), legacy_contents()).unwrap();

    disable_autostart().unwrap();

    assert!(!autostart_dir.join(LEGACY_DESKTOP_FILE).exists());
    assert!(!autostart_dir.join(DESKTOP_FILE).exists());
    assert_eq!(entry_count(&autostart_dir), 0);
}

// ── legacy migration ──────────────────────────────────────────────────────

#[test]
fn migrate_renames_legacy_when_new_identity_absent() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    std::fs::create_dir_all(&autostart_dir).unwrap();
    let contents = legacy_contents();
    std::fs::write(autostart_dir.join(LEGACY_DESKTOP_FILE), &contents).unwrap();

    migrate_legacy_autostart().unwrap();

    assert!(
        !autostart_dir.join(LEGACY_DESKTOP_FILE).exists(),
        "legacy entry must be gone after migration"
    );
    assert_eq!(
        std::fs::read_to_string(autostart_dir.join(DESKTOP_FILE)).unwrap(),
        contents,
        "rename must preserve the user's enabled state and Exec"
    );
}

#[test]
fn migrate_removes_legacy_when_both_exist() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();
    std::fs::create_dir_all(&autostart_dir).unwrap();
    std::fs::write(autostart_dir.join(LEGACY_DESKTOP_FILE), legacy_contents()).unwrap();
    std::fs::write(autostart_dir.join(DESKTOP_FILE), "new identity").unwrap();

    migrate_legacy_autostart().unwrap();

    assert!(
        !autostart_dir.join(LEGACY_DESKTOP_FILE).exists(),
        "the duplicate legacy entry must be removed"
    );
    assert_eq!(
        std::fs::read_to_string(autostart_dir.join(DESKTOP_FILE)).unwrap(),
        "new identity",
        "the new-identity entry wins when both exist"
    );
}

#[test]
fn migrate_is_a_noop_without_legacy_entry() {
    let (_guard, autostart_dir, _applications_dir) = isolated_dirs();

    migrate_legacy_autostart().unwrap();

    assert!(
        !autostart_dir.exists(),
        "migration must not create directories as a side effect"
    );
}

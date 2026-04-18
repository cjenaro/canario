# Known Bugs & Regressions

> These were introduced during the `canario-core` refactor. The backend is solid
> but the GTK frontend (`canario-gtk`) lost wiring in several places.

## Critical (app is broken without these)

### BUG-001: Tray menu actions are no-ops
**File:** `canario-gtk/src/ui/tray.rs`
**Severity:** Critical

The `CanarioTray` no longer has a channel to communicate back to the app.
Start/Stop, Settings, and Quit button closures are empty.

**Before (old `src/ui/tray.rs`):** Tray held a `Sender<AppMessage>` and sent
`ToggleRecording`, `ShowSettings`, `Quit` on menu clicks.

**Now:** Tray only holds an `Arc<AtomicBool>` for recording state display.
No `Sender` or callback mechanism.

**Fix:** Give `CanarioTray` a sender or callback that the `CanarioGtkApp`
event loop can process. Options:
- Add a `Sender<TrayAction>` enum to `CanarioTray`, poll it in the
  `glib::timeout_add_local` loop in `app.rs`, and call `canario.toggle_recording()`,
  open settings, or quit accordingly.
- Or use a shared `mpsc::Sender<Event>` — add new `Event` variants like
  `TrayToggleRecording`, `TrayShowSettings`, `TrayQuit` to `canario-core/src/event.rs`
  (or keep them GTK-side only).

---

### BUG-002: Settings window doesn't open on first launch
**File:** `canario-gtk/src/ui/app.rs`
**Severity:** Critical

When no model is downloaded, the app should auto-open Settings to prompt
the user to download one. The `connect_activate` closure has a comment
but no actual code to open the settings window.

**Before:** `AppMessage::ShowSettings` was sent, which called
`SettingsWindow::present()`.

**Now:** The `connect_activate` closure is empty.

**Fix:** In the `connect_activate` closure, call
`SettingsWindow::present(app, &canario)` when `!canario.is_model_downloaded()`.

---

### BUG-003: Model download progress not wired to UI
**File:** `canario-gtk/src/ui/app.rs`, `canario-gtk/src/ui/model_manager.rs`
**Severity:** High

`canario.download_model()` emits `Event::ModelDownloadProgress(f64)` which
`app.rs` handles by walking the widget tree to find a `ProgressBar`. This
may or may not work. The download button in `model_manager.rs` calls
`canario.download_model()` but the progress bar reference isn't shared.

**Before:** The model manager had a local `progress_bar` reference and a
`glib::timeout_add_local` polling a `result_rx` channel.

**Now:** Progress events go through the global event channel. The widget-tree
walker (`find_progress_bar`) searches by type which could match any
`ProgressBar` in the settings window.

**Fix:** Either:
- Use `gtk4::Widget::set_widget_name("model-download-progress")` on the
  progress bar in `model_manager.rs` and search by name instead of by type.
- Or have `model_manager.rs` subscribe to events directly (pass the event
  receiver or a secondary channel).

Also: `Event::ModelDownloadComplete` and `Event::ModelDownloadFailed`
handlers in `app.rs` need to update the status label and toggle button
visibility in the model manager. Currently `update_download_complete()`
only sets the progress bar to 1.0 but doesn't change "⬇ Downloading…" to
"✅ Ready" or show/hide the download/delete buttons.

---

### BUG-004: CLI gutted — lost all functionality except --toggle-external
**File:** `canario-cli/src/main.rs`
**Severity:** High

The old CLI (`src/bin/canario-cli.rs`) had ~600 lines with:
- `--download` (download models)
- `--wav <file>` (transcribe WAV)
- `--mic` (VAD streaming from mic)
- `--mic --paste` (auto-paste results)
- `--mic --toggle` (press Enter to start/stop)

The new CLI is ~30 lines and only supports `--toggle-external`.

**Fix:** Rewrite `canario-cli/src/main.rs` to use `canario-core` APIs:
- For `--download`: call `canario.download_model()` and poll events
- For `--wav`: use `canario_core::inference::TranscriptionEngine` (needs
  to be re-exported or the logic pulled out of recording.rs)
- For `--mic`: create a recording directly using the core recording module
  and poll the event receiver

Note: Some of this logic (VAD streaming, WAV reading) only exists in the
old CLI code. It needs to be moved into `canario-core` or the new CLI
needs to re-implement it using core primitives.

---

## Medium (degraded experience)

### BUG-005: Recording indicator shows but tray icon doesn't update
**File:** `canario-gtk/src/ui/app.rs`
**Severity:** Medium

When `Event::RecordingStarted` fires, `is_recording_flag` is set and
`refresh_tray()` is called. But the tray's `is_recording: Arc<AtomicBool>`
is a *different* `Arc` than the one the tray was created with.

**In `app.rs` `setup_signals()`:** `tx_tray` is `self.is_recording_flag.clone()`
and it's passed to `start_tray(flag)`. But `start_tray` creates
`CanarioTray::new(is_recording)` which stores it. Then in `handle_event`,
`is_recording_flag.store(true)` is called on a *third* clone.

This should actually work since `Arc::clone()` shares the same allocation.
But verify: the `is_recording_flag` in `setup_signals` and the one in
`handle_event` are the same `Arc`.

**Status:** Likely works, but verify. The `CanarioGtkApp` struct has one
`is_recording_flag` field that's cloned into both the startup closure and
the event handler via `self.is_recording_flag`.

---

### BUG-006: Word remapping "Add" button does nothing
**File:** `canario-gtk/src/ui/word_remapping.rs`
**Severity:** Medium

The `add_btn` is created but never connected. There's a `let _ = add_btn; // TODO`
comment. The old version had a full dialog with `adw::Dialog`, mode selector
(remapping vs removal), and entry rows.

**Fix:** Port the dialog from the old `src/ui/word_remapping.rs`. Use
`canario.update_config()` to save changes when a rule is added.

---

### BUG-007: Word remapping delete buttons do nothing
**File:** `canario-gtk/src/ui/word_remapping.rs`
**Severity:** Medium

The remapping/removal rows don't have delete buttons anymore. The old
version had per-row delete buttons that called `config.post_processor.remappings.retain()`.

**Fix:** Add delete buttons to `make_remapping_row()` and `make_removal_row()`
that call `canario.update_config()` to remove the rule.

---

### BUG-008: History list doesn't refresh when re-opening Settings
**File:** `canario-gtk/src/ui/history.rs`
**Severity:** Medium

The history widget populates once at construction time. If you record
something, close Settings, then re-open it, the new transcription won't
appear because the old `refresh_history_in_window` + `find_widget_by_name`
logic was removed.

**Fix:** Either:
- Re-add the widget-tree-walker approach from the old `settings.rs`
- Or make `HistoryWidget` store the `Canario` handle and repopulate in a
  `map()` or `show()` callback

---

### BUG-009: History search bar removed
**File:** `canario-gtk/src/ui/history.rs`
**Severity:** Low

The old `HistoryWidget` had a `SearchEntry` for filtering entries by text.
The new version doesn't have search.

**Fix:** Add a `gtk4::SearchEntry` above the list, connect
`connect_search_changed` to call `canario.search_history()`.

---

### BUG-010: Sound effects — stop beep not played
**File:** `canario-core/src/recording.rs`
**Severity:** Low

`beep_start()` is played when recording starts and `beep_confirm()` is
played after successful paste. But `beep_stop()` (the double-beep) is
never called — it should play when the recording stops (before
transcription begins).

**Fix:** Call `crate::audio::effects::beep_stop()` at the beginning of
the post-stop transcription section in `recording_loop()`, gated by
a `sound_effects` flag that needs to be passed through (currently only
`start_recording` receives it, but the stop happens in the loop itself).

---

## Cleanup (not bugs, but should be addressed)

### CLEANUP-001: Dead code in canario-core
**Files:** `canario-core/src/audio/mod.rs`, `canario-core/src/inference/mod.rs`

`AudioCapture` (ring buffer-based capture used by the old CLI),
`TranscriptionEngine`, `read_wav`, `download_model` (without progress),
`save_wav` are all unused. Either remove them or re-export for CLI use
(see BUG-004).

### CLEANUP-002: Doc-tests marked `no_run` need fixing
**Files:** `canario-core/src/lib.rs`, `canario-core/src/canario.rs`

The inline doc examples have `no_run` to avoid compilation errors.
Once the API is stable, fix them to be valid runnable tests.

### CLEANUP-003: packaging scripts reference old build paths
**File:** `packaging/build-deb.sh`, `packaging/build-appimage.sh`

The scripts reference `target/release/canario` and `target/release/canario-cli`
which still work in a workspace, but the `.desktop` file and icon paths
reference the old `assets/` directory at the repo root. Verify the
`assets/canario.svg` and `assets/com.canario.Canario.desktop` are still
at the right relative path from the workspace root.

### CLEANUP-004: Old `src/` directory deleted but old model_manager had progress download logic
The old `src/ui/model_manager.rs` had a self-contained download with
`glib::timeout_add_local` polling a `result_rx`. The new version delegates
to `canario.download_model()` which sends events globally. This is cleaner
but the UI update path is fragile (see BUG-003).

//! Warm microphone capture — one parked capture stream, reused across
//! dictations (bead canario-vew).
//!
//! # Why
//!
//! Every recording used to reopen the input device and stream
//! (`default_input_device` + `build_input_stream` + `play`), and the
//! dominant cost was the wait for the first sample callback after
//! `play` — measured 48.8–50.3 ms mean press→first-audio (n=10). A
//! stream that stays *running* between dictations removes all of that:
//! the audio callbacks never stop, so a press only has to mark where
//! the new recording starts.
//!
//! # Design
//!
//! * One [`cpal::Stream`] per process writes mono samples into a
//!   global bounded [`RingBuffer`] addressed by an absolute
//!   `written` counter (samples ever captured). The ring only needs to
//!   cover the drain interval (~50 ms), so it is a few seconds deep —
//!   while parked it simply wraps forever.
//! * A press calls [`begin_recording`]: reuse the parked stream when
//!   healthy (µs), else open a fresh one inline (the pre-vew cold
//!   path, ~15 ms, marked `mic_device_opened`/`mic_stream_started`).
//!   Either way the recording's `start_offset` is the ring's write
//!   position at that moment, and the loop drains `[offset, written)`
//!   into its own buffer every tick.
//! * Device selection (canario-1hq.2): a process-wide preferred
//!   input name ([`set_preferred_device`], fed from
//!   `AppConfig.input_device` by the backend whenever the config
//!   loads or changes — the recording pipeline itself never changes).
//!   [`begin_recording`] opens it when set, else the system default,
//!   falling back to the default (with a warning) when the preferred
//!   device is missing at open time. A parked stream opened for a
//!   different selection than now wanted is released, not reused —
//!   mirroring the park-time default-identity check below.
//! * The capture period is pinned small ([`PREFERRED_PERIOD_FRAMES`]):
//!   a warm press waits out the *remaining* period before its first
//!   block arrives, and the host default (~2048 frames ≈ 43 ms here)
//!   alone would exceed the latency target. Devices that reject the
//!   small period fall back to the default.
//! * `first_audio` stays honest: the threshold is armed at the press
//!   offset and the data callback marks the first block that crosses
//!   it — the first sample *after* the offset, whether the stream is
//!   fresh (first block after `play`) or parked (next block after the
//!   arm, ≤ one callback period).
//! * Stop keeps the canario-b1g condvar wake; the loop does one final
//!   drain after waking so no tail audio is lost to the tick cadence.
//!
//! # Warmth window (the documented tradeoff)
//!
//! A permanently-open stream would keep the desktop's mic-indicator
//! lit whenever the app idles. Instead the stream is *parked* for
//! [`PARK_WINDOW`] (60 s) after the last recording and then fully
//! released by the mic-owner thread's idle watch (`mic_idle_released`
//! mark). Rapid dictations stay warm; the indicator is dark at
//! startup and at most 60 s after the last dictation.
//!
//! # The mic-owner thread (canario-2z0)
//!
//! Exactly one thread — the owner, spawned lazily on first use —
//! creates, parks, reopens, and drops the [`cpal::Stream`]. Everyone
//! else talks to it over a command channel. This is a portability
//! requirement, not a style choice: on macOS cpal's coreaudio
//! `Stream` is `!Send` (it embeds a `Box<dyn FnMut()>`
//! property-listener wrapper), so the Stream cannot live in shared
//! state, a `static`, or cross a `thread::spawn`. Keeping it as the
//! owner's plain local makes the module compile on every platform
//! and structurally guarantees streams are never dropped under the
//! state lock (cpal joins its audio thread on drop).
//!
//! # Failure handling (correctness over warmth)
//!
//! * The error callback bumps a counter. A stream that has ever
//!   errored is never reused — the next press pays the inline open.
//! * Mid-recording, an error *plus* 500 ms of no samples (device
//!   unplugged, hotplug) triggers [`MicSession::reopen_after_error`]:
//!   the failed stream is dropped and a fresh one continues into the
//!   same ring (offsets stay valid across the swap), at the cost of a
//!   short audio gap. One reopen per recording; a second failure
//!   aborts and transcribes the partial audio.
//! * At park time the wanted device is re-checked: with a preferred
//!   device set, the parked stream must have been capturing from that
//!   same device; in system-default mode the default input device is
//!   re-queried, and if it no longer matches the captured device
//!   (default-source switch while parked) the stream is released
//!   instead — so a stale device is captured for at most one
//!   dictation.
//!
//! # Timing marks
//!
//! `mic_warm_reused` (press reused the parked stream),
//! `mic_idle_released` (the owner's idle watch released the mic after
//! the window); the cold path keeps `mic_device_opened`,
//! `mic_stream_started` and `first_audio` with their original
//! meanings.

use std::ops::Range;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;

use crate::timing;

/// How long the stream stays parked after the last recording before
/// the owner thread's idle watch releases it (mic indicator off,
/// device closed).
///
/// 60 s comfortably covers rapid back-to-back dictations while
/// bounding idle indicator exposure. Tradeoff is documented in the
/// module docs and the bead notes.
const PARK_WINDOW: Duration = Duration::from_secs(60);

/// Ring depth in seconds of audio. The capture loop drains every
/// ~50 ms, so 5 s is a ~100× safety margin against scheduling
/// stalls; overrun is still detected and logged (see
/// [`clamped_drain_range`]).
const RING_SECS: f64 = 5.0;

/// A stalled stream counts as failed once no sample has arrived for
/// this long *and* the error callback has fired. Generous on purpose:
/// the capture period is tens of ms, so 500 ms of total silence means
/// the device is really gone, not merely glitching.
const MID_RECORDING_STALL_MS: u64 = 500;

/// Mid-recording reopen attempts before giving up (the recording is
/// aborted and the partial audio transcribed).
pub(crate) const MAX_REOPENS_PER_RECORDING: u32 = 1;

/// Sentinel for "first_audio not armed" — no real `written` value can
/// reach it.
const DISARMED: u64 = u64::MAX;

// ── Ring buffer ───────────────────────────────────────────────────────

/// Bounded mono-sample ring addressed by absolute sample index.
///
/// `written` counts every sample ever appended (never resets, even
/// when streams are swapped or the ring is resized), which is what
/// makes press offsets and drain cursors stable across the process
/// lifetime. Samples older than `written - capacity` are gone.
struct RingBuffer {
    data: Vec<f32>,
    written: u64,
}

impl RingBuffer {
    fn written(&self) -> u64 {
        self.written
    }

    fn capacity(&self) -> u64 {
        self.data.len() as u64
    }

    /// Oldest sample index still retrievable.
    fn oldest_available(&self) -> u64 {
        self.written.saturating_sub(self.capacity())
    }

    /// Append a block, wrapping (or dropping the oldest end of an
    /// oversized block). Unallocated rings must not be pushed to —
    /// see [`Self::ensure_capacity_for`]; the early return below
    /// keeps offsets sane rather than panicking on the audio thread
    /// if that invariant is ever broken.
    fn push(&mut self, chunk: &[f32]) {
        if chunk.is_empty() {
            return;
        }
        let cap = self.data.len();
        if cap == 0 {
            self.written += chunk.len() as u64;
            return;
        }
        if chunk.len() >= cap {
            // Keep only the newest `cap` samples, laid out exactly as
            // if they had been pushed one by one (sample i lives at
            // i % cap) so later reads stay in capture order. The kept
            // tail begins at absolute index `written + len - cap`.
            let tail = &chunk[chunk.len() - cap..];
            let first = ((self.written + (chunk.len() - cap) as u64) % cap as u64) as usize;
            self.data[first..].copy_from_slice(&tail[..cap - first]);
            self.data[..first].copy_from_slice(&tail[cap - first..]);
        } else {
            let start = (self.written % cap as u64) as usize;
            let contiguous = cap - start;
            if chunk.len() <= contiguous {
                self.data[start..start + chunk.len()].copy_from_slice(chunk);
            } else {
                self.data[start..].copy_from_slice(&chunk[..contiguous]);
                let rest = chunk.len() - contiguous;
                self.data[..rest].copy_from_slice(&chunk[contiguous..]);
            }
        }
        self.written += chunk.len() as u64;
    }

    /// Copy out `[range.start, range.end)`, clamped to what is still
    /// available. Returns the samples in capture order even across
    /// the wrap point.
    fn take_range(&self, range: Range<u64>) -> Vec<f32> {
        let oldest = self.oldest_available();
        let from = range.start.clamp(oldest, self.written);
        let to = range.end.clamp(from, self.written);
        let mut out = Vec::with_capacity((to - from) as usize);
        if to > from {
            let cap = self.data.len() as u64;
            let start_idx = (from % cap) as usize;
            let contiguous = (cap - from % cap) as usize;
            let len = (to - from) as usize;
            if len <= contiguous {
                out.extend_from_slice(&self.data[start_idx..start_idx + len]);
            } else {
                out.extend_from_slice(&self.data[start_idx..]);
                out.extend_from_slice(&self.data[..len - contiguous]);
            }
        }
        out
    }

    /// (Re)size for `sample_rate`, preserving `written` so live drain
    /// cursors survive a mid-process rate change (device switch).
    fn ensure_capacity_for(&mut self, sample_rate: u32, secs: f64) {
        let want = (sample_rate as f64 * secs).max(1.0) as usize;
        if want != self.data.len() {
            self.data = vec![0.0; want];
        }
    }
}

/// The drain span for a cursor that may have fallen behind the ring:
/// the span plus whether it had to be clamped (drain consumer stalled
/// longer than the ring depth — audio was lost).
fn clamped_drain_range(taken: u64, written: u64, capacity: u64) -> (Range<u64>, bool) {
    let oldest = written.saturating_sub(capacity);
    if taken < oldest {
        (oldest..written, true)
    } else {
        (taken..written, false)
    }
}

// ── Shared state ──────────────────────────────────────────────────────

/// Lifecycle of the parked stream.
enum Phase {
    /// No stream; the mic is closed and the indicator dark.
    Released,
    /// A recording owns the stream right now.
    Recording,
    /// Stream alive but idle; the owner thread's idle watch releases
    /// it at `deadline`.
    Parked { deadline: Instant },
}

/// Identity of the device a stream captures from — used by the park
/// decision to notice a default-source switch.
#[derive(Clone, PartialEq, Eq, Debug)]
struct DeviceIdentity {
    name: String,
    sample_rate: u32,
    channels: u16,
}

impl DeviceIdentity {
    fn of(device: &cpal::Device, supported: &cpal::SupportedStreamConfig) -> Self {
        Self {
            name: device.name().unwrap_or_default(),
            sample_rate: supported.sample_rate().0,
            channels: supported.channels(),
        }
    }
}

// ── Input device enumeration (canario-1hq.2) ─────────────────────────

/// An enumerated audio input device, identified by its host name —
/// the settings picker's whole vocabulary. Names are what device
/// selection matches on everywhere in this module.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MicDevice {
    pub name: String,
}

/// Enumerate the host's input devices, deduplicated by name (first
/// occurrence wins — hosts can expose one physical device more than
/// once). An enumeration failure degrades to an empty list: callers
/// surface "no devices" instead of an error, so a missing audio
/// subsystem never breaks the picker.
pub fn list_input_devices() -> Vec<MicDevice> {
    enumerate_input_devices(&cpal::default_host())
}

fn enumerate_input_devices(host: &cpal::Host) -> Vec<MicDevice> {
    match host.input_devices() {
        Ok(devices) => deduped_devices(devices.filter_map(|d| d.name().ok())),
        Err(e) => {
            tracing::warn!("Could not enumerate input devices: {}", e);
            Vec::new()
        }
    }
}

/// Deduplicate device names, first occurrence winning, dropping
/// empty names (a device whose name cannot be read or is blank
/// cannot be selected by name anyway).
fn deduped_devices(names: impl Iterator<Item = String>) -> Vec<MicDevice> {
    let mut seen = std::collections::BTreeSet::new();
    names
        .filter(|name| !name.is_empty() && seen.insert(name.clone()))
        .map(|name| MicDevice { name })
        .collect()
}

// ── Process-wide preferred device (canario-1hq.2) ────────────────────

/// The preferred input device name, process-wide. `Canario` pushes
/// `AppConfig.input_device` here whenever the config loads or changes
/// (see `Canario::new` / `update_config` / `refresh_config`), so a
/// settings change takes effect on the next recording without a
/// restart — [`begin_recording`] reads it here, which is why the
/// recording pipeline needs no changes of its own. `None`/blank means
/// the system default (the pre-picker behavior). Same shape as
/// `transform`'s process-wide credential store (fgm.3).
static PREFERRED_DEVICE: std::sync::OnceLock<parking_lot::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

/// Serialize every test that touches the process-wide store — Rust
/// runs unit tests on parallel threads, and a store is exactly the
/// kind of state they would clobber (mirrors the sidecar's
/// CREDENTIAL_TEST_LOCK).
#[cfg(test)]
pub(crate) static PREFERRED_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Store (`Some` non-blank) or clear (`None`/blank) the preferred
/// input device. The value is trimmed: a whitespace-only config value
/// means "system default", not a name to match.
pub fn set_preferred_device(name: Option<String>) {
    let lock = PREFERRED_DEVICE.get_or_init(|| parking_lot::Mutex::new(None));
    *lock.lock() = name.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
}

/// Clone of the preferred device name, if one is set.
pub fn preferred_device() -> Option<String> {
    PREFERRED_DEVICE.get().and_then(|lock| lock.lock().clone())
}

/// Which input device a recording wants: the process-wide preferred
/// device, or the system default.
#[derive(Clone, PartialEq, Eq, Debug)]
enum DeviceSelection {
    /// The named preferred device (from `AppConfig.input_device`).
    Preferred(String),
    /// The system default input device (the pre-picker behavior).
    SystemDefault,
}

/// The selection the next recording wants, mapped from the
/// process-wide preference (empty/absent = system default).
fn current_selection() -> DeviceSelection {
    match preferred_device() {
        Some(name) => DeviceSelection::Preferred(name),
        None => DeviceSelection::SystemDefault,
    }
}

/// Identity of the current default input device, if one exists.
fn default_input_identity() -> Option<DeviceIdentity> {
    let device = cpal::default_host().default_input_device()?;
    let supported = device.default_input_config().ok()?;
    Some(DeviceIdentity::of(&device, &supported))
}

/// Everything guarded by the state mutex. The data/error callbacks
/// and the recording thread take this lock briefly.
///
/// Deliberately Stream-free (canario-2z0): cpal's coreaudio `Stream`
/// is `!Send` (it embeds a `Box<dyn FnMut()>` property-listener
/// wrapper), so a `Stream` cannot live in this shared state — it is
/// held only as a local by the dedicated mic-owner thread
/// ([`owner_loop`]), which is also the only code that drops it. On
/// Linux/Windows the `Stream` is `Send`, but the owner keeps one code
/// shape across platforms. A pleasant side effect: streams are never
/// dropped while this lock is held, because no lock is held on the
/// owner thread across a drop at all.
struct WarmState {
    ring: RingBuffer,
    stream_identity: Option<DeviceIdentity>,
    /// Which device selection the live stream was opened for — the
    /// press-time reuse check (a stream parked for another selection
    /// than now wanted must be released, not reused) and the
    /// preferred-mode park check both read it (canario-1hq.2).
    stream_selection: Option<DeviceSelection>,
    phase: Phase,
    /// Cumulative error-callback count for the live stream (reset when
    /// a stream is stored).
    error_count: u64,
    /// `written` value whose crossing by a data block marks
    /// `first_audio`; `DISARMED` while parked/stopped.
    first_audio_at: u64,
    first_audio_marked: bool,
}

impl WarmState {
    fn new() -> Self {
        Self {
            ring: RingBuffer {
                data: Vec::new(),
                written: 0,
            },
            stream_identity: None,
            stream_selection: None,
            phase: Phase::Released,
            error_count: 0,
            first_audio_at: DISARMED,
            first_audio_marked: false,
        }
    }
}

/// State plus config, shared with the recording session and the
/// mic-owner thread. The park deadline lives in
/// [`Phase::Parked`] inside the state, so the owner's idle wait and
/// every observer read one source of truth.
struct SharedMic {
    state: Arc<Mutex<WarmState>>,
    window: Duration,
    ring_secs: f64,
}

impl SharedMic {
    /// The state mutex as a fresh `Arc` handle, for capture callbacks.
    fn state_arc(self: &Arc<Self>) -> Arc<Mutex<WarmState>> {
        Arc::clone(&self.state)
    }
}

/// The warm-mic machinery. One instance lives on the mic-owner thread
/// ([`owner_loop`]); tests drive its owner-side methods directly.
struct WarmMic {
    shared: Arc<SharedMic>,
}

impl WarmMic {
    fn new(window: Duration, ring_secs: f64) -> Self {
        Self {
            shared: Arc::new(SharedMic {
                state: Arc::new(Mutex::new(WarmState::new())),
                window,
                ring_secs,
            }),
        }
    }
}

// ── Mic-owner thread (canario-2z0) ─────────────────────────────────────
//
// The `cpal::Stream` lives on exactly one thread for the whole
// process: a dedicated owner, created lazily on first use. Callers
// (the recording thread) talk to it over a command channel and get a
// reply channel back per call — the only cross-thread value is the
// `Sender`, which is `Send` on every platform. This is what makes the
// module compile on macOS, where cpal's coreaudio `Stream` is `!Send`
// (it embeds a `Box<dyn FnMut()>` property-listener wrapper): the
// Stream never crosses a thread boundary, never enters shared state,
// and never meets a `static`'s `Sync` bound. The owner's
// `recv_timeout` doubles as the park reaper — the idle wait IS the
// watch — which also retires the old condvar/reaper pair.

/// A reply channel for a blocking owner round-trip. `sync_channel(1)`
/// so a dropped caller never blocks the owner.
type Reply<T> = std::sync::mpsc::SyncSender<anyhow::Result<T>>;

enum OwnerCmd {
    /// Acquire the mic for a recording (reuse the parked stream when
    /// healthy and wanted, else open fresh). Replies the press offset
    /// and sample rate.
    Begin { reply: Reply<BeginInfo> },
    /// Mid-recording recovery: drop the failed stream and open a fresh
    /// one continuing into the same ring.
    Reopen { reply: Reply<()> },
    /// End of recording: park for the idle window or release now.
    Finish { reply: Reply<()> },
}

/// What a successful `Begin` hands back to the recording thread.
struct BeginInfo {
    sample_rate: u32,
    start_offset: u64,
}

/// Handle to the process's mic-owner thread. `Send + Sync` on every
/// platform (a plain channel sender plus a plain state handle), so
/// the static is fine.
struct MicOwnerHandle {
    tx: std::sync::mpsc::Sender<OwnerCmd>,
    shared: Arc<SharedMic>,
}

impl MicOwnerHandle {
    /// Blocking begin round-trip. The cold path's device-open cost is
    /// paid here either way (the owner opens it); the warm path costs
    /// one channel hop (tens of µs).
    fn begin(&self) -> anyhow::Result<MicSession> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.tx
            .send(OwnerCmd::Begin { reply: tx })
            .map_err(|_| anyhow::anyhow!("mic owner thread is gone"))?;
        let BeginInfo {
            sample_rate,
            start_offset,
        } = rx
            .recv()
            .map_err(|_| anyhow::anyhow!("mic owner thread is gone"))??;
        Ok(MicSession {
            shared: Arc::clone(&self.shared),
            sample_rate,
            start_offset,
            errors_at_begin: 0,
            finished: false,
            owner: Some(self.tx.clone()),
        })
    }
}

/// The owner thread's loop. Holds the one and only `cpal::Stream` as a
/// plain local, parks or releases it per command, and treats a
/// lapsed park deadline (its `recv_timeout`) as the release trigger.
fn owner_loop(rx: std::sync::mpsc::Receiver<OwnerCmd>, mic: WarmMic) {
    let mut stream: Option<cpal::Stream> = None;
    loop {
        // Parked → wake at the deadline; anything else (recording,
        // released) → wait long enough that the thread is idle noise.
        let timeout = match mic.shared.state.lock().phase {
            Phase::Parked { deadline } => deadline.saturating_duration_since(Instant::now()),
            _ => Duration::from_secs(3600),
        };
        if timeout.is_zero() {
            // Deadline already lapsed between commands.
            stream = None;
            mic.release_for_idle();
            continue;
        }
        match rx.recv_timeout(timeout) {
            Ok(OwnerCmd::Begin { reply }) => {
                let res = mic.begin_owned(&mut stream);
                let _ = reply.send(res);
            }
            Ok(OwnerCmd::Reopen { reply }) => {
                let res = mic.reopen_owned(&mut stream);
                let _ = reply.send(res);
            }
            Ok(OwnerCmd::Finish { reply }) => {
                mic.finish_owned(&mut stream);
                let _ = reply.send(Ok(()));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // The park window elapsed with no new command: fully
                // release the stream (mic indicator off, device closed).
                stream = None;
                mic.release_for_idle();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Every handle dropped (process teardown): release and
                // stop. Dropping the stream joins cpal's audio thread.
                drop(stream);
                return;
            }
        }
    }
}

/// Process-global mic owner. First use spawns the thread; it lives
/// until every handle (including the static) goes away, which in
/// practice means process exit.
fn owner() -> &'static MicOwnerHandle {
    static OWNER: LazyLock<MicOwnerHandle> = LazyLock::new(|| {
        let mic = WarmMic::new(PARK_WINDOW, RING_SECS);
        let shared = Arc::clone(&mic.shared);
        let (tx, rx) = std::sync::mpsc::channel::<OwnerCmd>();
        std::thread::Builder::new()
            .name("mic-warm-owner".to_string())
            .spawn(move || owner_loop(rx, mic))
            .expect("failed to spawn the mic-owner thread");
        MicOwnerHandle { tx, shared }
    });
    &OWNER
}

/// Acquire the mic for a recording: reuse the parked stream when
/// healthy (marks `mic_warm_reused`), else open a fresh one on the
/// owner thread (marks `mic_device_opened` + `mic_stream_started`,
/// the pre-vew path). Arms `first_audio` at the current write
/// position either way.
pub(crate) fn begin_recording() -> anyhow::Result<MicSession> {
    owner().begin()
}

impl WarmMic {
    /// Owner-side begin (canario-2z0): `stream` is the owner thread's
    /// local slot. Reuse the parked stream when healthy and opened for
    /// the selection now wanted, else release it and cold-open the
    /// wanted device — every decision identical to the pre-2z0 logic,
    /// only the stream's home changed.
    fn begin_owned(&self, stream: &mut Option<cpal::Stream>) -> anyhow::Result<BeginInfo> {
        // Fast path — a healthy parked stream opened for the device now
        // wanted: arm and go (µs). The arm runs under the state lock so a
        // concurrent data block cannot slip between the threshold store
        // and the flag reset.
        let wanted = current_selection();
        let reused = {
            let mut st = self.shared.state.lock();
            if begin_verdict(
                stream.is_some(),
                st.error_count > 0,
                st.stream_selection.as_ref(),
                &wanted,
            ) == BeginVerdict::Reuse
            {
                let offset = st.ring.written();
                st.first_audio_marked = false;
                st.first_audio_at = offset;
                st.phase = Phase::Recording;
                st.stream_identity
                    .as_ref()
                    .map(|id| (offset, id.sample_rate))
            } else {
                None
            }
        };
        if let Some((offset, sample_rate)) = reused {
            timing::mark("mic_warm_reused");
            tracing::info!(
                "Recording via parked mic stream at {}Hz (warm)",
                sample_rate
            );
            return Ok(BeginInfo {
                sample_rate,
                start_offset: offset,
            });
        }

        // A parked stream we must not reuse (errored, or opened for a
        // different device than the one now wanted — e.g. the
        // preference changed while parked) is released here. The owner
        // holds no state lock across the drop (cpal joins its audio
        // thread on drop), so the discipline is structural now.
        let wrong_device = {
            let st = self.shared.state.lock();
            stream.is_some() && st.error_count == 0 && st.stream_selection.as_ref() != Some(&wanted)
        };
        if wrong_device {
            tracing::info!(
                "Parked mic stream was opened for a different device — \
                 releasing it and opening the wanted one"
            );
        } else if stream.is_some() {
            tracing::info!("Parked mic stream was unhealthy — reopening input device");
        }
        *stream = None;

        // Cold open, paid by the recording thread's blocking round-trip
        // to this owner thread. Holding the state lock across the open
        // is fine: no stream exists, so no data callback can contend.
        let mut st = self.shared.state.lock();
        let host = cpal::default_host();
        let plan = resolve_device_plan(&wanted, &enumerate_input_devices(&host));
        let device = open_input_device(&host, &plan)
            .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
        let supported = device.default_input_config()?;
        let identity = DeviceIdentity::of(&device, &supported);
        let sample_rate = supported.sample_rate().0;
        tracing::info!("Recording from '{}' at {}Hz", identity.name, sample_rate);
        timing::mark("mic_device_opened");

        let new_stream = build_ring_stream(&device, &supported, self.shared.state_arc())?;
        st.ring
            .ensure_capacity_for(sample_rate, self.shared.ring_secs);
        // Arm before play: the first post-play block must be the one
        // that marks `first_audio` (pre-vew semantics).
        let offset = st.ring.written();
        st.first_audio_marked = false;
        st.first_audio_at = offset;
        st.error_count = 0;
        st.stream_identity = Some(identity);
        st.stream_selection = Some(selection_of_plan(&plan));
        st.phase = Phase::Recording;
        let played = new_stream.play();
        drop(st); // never hold the state lock across a stream drop
        if let Err(e) = played {
            drop(new_stream);
            return Err(e.into());
        }
        *stream = Some(new_stream);
        timing::mark("mic_stream_started");

        Ok(BeginInfo {
            sample_rate,
            start_offset: offset,
        })
    }

    /// Owner-side mid-recording recovery: drop the failed stream and
    /// open a fresh one continuing into the same ring. The owner holds
    /// no state lock across the drop (cpal joins the audio thread).
    fn reopen_owned(&self, stream: &mut Option<cpal::Stream>) -> anyhow::Result<()> {
        *stream = None;

        let mut st = self.shared.state.lock();
        let host = cpal::default_host();
        let wanted = current_selection();
        let plan = resolve_device_plan(&wanted, &enumerate_input_devices(&host));
        let device = open_input_device(&host, &plan)
            .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
        let supported = device.default_input_config()?;
        let identity = DeviceIdentity::of(&device, &supported);
        let new_stream = build_ring_stream(&device, &supported, self.shared.state_arc())?;
        st.ring
            .ensure_capacity_for(identity.sample_rate, self.shared.ring_secs);
        st.stream_identity = Some(identity);
        st.stream_selection = Some(selection_of_plan(&plan));
        st.error_count = 0;
        let played = new_stream.play();
        drop(st); // never hold the state lock across a stream drop
        if let Err(e) = played {
            drop(new_stream);
            return Err(e.into());
        }
        *stream = Some(new_stream);
        Ok(())
    }

    /// Owner-side end of recording: disarm `first_audio`, then either
    /// park for the idle window (the owner's idle wait — its
    /// `recv_timeout` on the deadline in [`Phase::Parked`] — releases
    /// after the window) or release now when the wanted device no
    /// longer matches the captured one.
    fn finish_owned(&self, stream: &mut Option<cpal::Stream>) {
        let keep = {
            let mut st = self.shared.state.lock();
            st.first_audio_at = DISARMED;
            let keep = stream.is_some()
                && park_decision(
                    st.stream_identity.as_ref(),
                    &current_selection(),
                    default_input_identity().as_ref(),
                );
            if keep {
                st.phase = Phase::Parked {
                    deadline: Instant::now() + self.shared.window,
                };
            } else {
                st.stream_identity = None;
                st.stream_selection = None;
                st.phase = Phase::Released;
            }
            keep
        };
        if !keep {
            *stream = None; // dropped here, no lock held: cpal joins the audio thread
        }
    }

    /// The idle watch fired: fully release the parked stream (mic
    /// indicator off, device closed). Called only from the owner loop.
    fn release_for_idle(&self) {
        {
            let mut st = self.shared.state.lock();
            st.stream_identity = None;
            st.stream_selection = None;
            st.phase = Phase::Released;
        }
        timing::mark("mic_idle_released");
        tracing::info!("Warm mic idle window elapsed — capture stream released (mic off)");
    }
}

// ── Recording session ─────────────────────────────────────────────────

/// A recording's handle on the warm mic: the press offset, the drain
/// cursor API, mid-recording error recovery, and the park-on-finish
/// transition. `finish` is idempotent; `Drop` finishes too, so a
/// panic between begin and finish still parks (and the owner still
/// releases after the window). Reads (drain/written/errors) go
/// straight to the shared state; lifecycle ops round-trip the
/// mic-owner thread (canario-2z0). The `owner` sender is `None` only
/// for tests driving the state directly.
pub(crate) struct MicSession {
    shared: Arc<SharedMic>,
    sample_rate: u32,
    start_offset: u64,
    errors_at_begin: u64,
    finished: bool,
    /// Channel to the mic-owner thread; `None` in local/test mode.
    owner: Option<std::sync::mpsc::Sender<OwnerCmd>>,
}

impl MicSession {
    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Absolute ring index this recording starts from — the press
    /// offset. Samples before it belong to nobody.
    pub(crate) fn start_offset(&self) -> u64 {
        self.start_offset
    }

    /// Total samples captured so far (stall detection).
    pub(crate) fn written(&self) -> u64 {
        self.shared.state.lock().ring.written()
    }

    /// Errors observed since the recording began.
    pub(crate) fn error_delta(&self) -> u64 {
        let now = self.shared.state.lock().error_count;
        now.saturating_sub(self.errors_at_begin)
    }

    /// Drain samples captured since the cursor into a Vec (capture
    /// order). Returns `(chunk, overran)` — `overran` means the cursor
    /// fell behind the ring depth and early audio was lost.
    pub(crate) fn drain_new(&self, taken: &mut u64) -> (Vec<f32>, bool) {
        let st = self.shared.state.lock();
        let (range, overran) = clamped_drain_range(*taken, st.ring.written(), st.ring.capacity());
        let chunk = st.ring.take_range(range.clone());
        *taken = range.end;
        (chunk, overran)
    }

    /// Mid-recording fallback: the stream failed. The owner thread
    /// drops it and opens a fresh one continuing into the same ring —
    /// absolute offsets are preserved, so the drain cursor and any
    /// unmarked `first_audio` threshold stay valid. Costs a short
    /// audio gap (the reopen).
    pub(crate) fn reopen_after_error(&self) -> anyhow::Result<()> {
        tracing::warn!(
            "Mic stream failed mid-recording — reopening the input device \
             (short gap in captured audio)"
        );
        let Some(owner) = &self.owner else {
            // Local/test sessions own no stream to recover.
            anyhow::bail!("mic owner unavailable — cannot reopen");
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        owner
            .send(OwnerCmd::Reopen { reply: tx })
            .map_err(|_| anyhow::anyhow!("mic owner thread is gone"))?;
        rx.recv()
            .map_err(|_| anyhow::anyhow!("mic owner thread is gone"))?
    }

    /// End of recording: disarm `first_audio`, then either park for
    /// the idle window (the owner's idle watch releases it after the
    /// window) or release now when the wanted device no longer matches
    /// the captured one (with a preferred device: it must be that
    /// device; in system-default mode: the default input device must
    /// still match).
    pub(crate) fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;

        match &self.owner {
            Some(owner) => {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                // Fire-and-forget on a dead owner is correct: with the
                // owner gone there is no stream left to park.
                if owner.send(OwnerCmd::Finish { reply: tx }).is_ok() {
                    let _ = rx.recv();
                }
            }
            // Local/test mode: no stream exists, so finish is just the
            // disarm + release bookkeeping the owner would have done.
            None => {
                let mut st = self.shared.state.lock();
                st.first_audio_at = DISARMED;
                st.stream_identity = None;
                st.stream_selection = None;
                st.phase = Phase::Released;
            }
        }
    }
}

impl Drop for MicSession {
    fn drop(&mut self) {
        self.finish();
    }
}

// ── Decisions (pure — unit-testable seams) ────────────────────────────

/// Should the stream stay parked after a recording? With a preferred
/// device set, only while the parked stream captures from that exact
/// device (the system default is irrelevant — a default-source switch
/// must not release a stream the user explicitly chose). In
/// system-default mode, only while the default input device still
/// matches the captured one — a hotplug or default-source switch must
/// not capture from a stale device forever.
fn park_decision(
    parked: Option<&DeviceIdentity>,
    wanted: &DeviceSelection,
    current_default: Option<&DeviceIdentity>,
) -> bool {
    let Some(parked) = parked else { return false };
    match wanted {
        DeviceSelection::Preferred(name) => parked.name == *name,
        DeviceSelection::SystemDefault => current_default == Some(parked),
    }
}

/// What to do with a parked stream at press time: reuse it (µs) or
/// release it and open the wanted device fresh. "Healthy" means a
/// stream exists and has never errored; a stream opened for a
/// DIFFERENT selection than now wanted (the preference changed while
/// parked) must not be reused — the recording would capture from the
/// wrong device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeginVerdict {
    /// Reuse the parked stream.
    Reuse,
    /// Release it (if any) and open the wanted device inline.
    Release,
}

fn begin_verdict(
    parked: bool,
    errored: bool,
    opened_for: Option<&DeviceSelection>,
    wanted: &DeviceSelection,
) -> BeginVerdict {
    if parked && !errored && opened_for == Some(wanted) {
        BeginVerdict::Reuse
    } else {
        BeginVerdict::Release
    }
}

/// Which concrete device to open for `wanted`, given the available
/// (enumerated) devices — the pure half of the cold open. A preferred
/// device that is missing at open time falls back to the system
/// default (`because_missing` drives the warning log): correctness
/// over strictness, dictation must never fail because a USB mic was
/// unplugged.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DevicePlan {
    /// Open the system default input device. `because_missing` names
    /// the preferred device this falls back FROM, if any.
    Default { because_missing: Option<String> },
    /// Open the named device.
    Named(String),
}

fn resolve_device_plan(wanted: &DeviceSelection, available: &[MicDevice]) -> DevicePlan {
    match wanted {
        DeviceSelection::SystemDefault => DevicePlan::Default {
            because_missing: None,
        },
        DeviceSelection::Preferred(name) => {
            if available.iter().any(|d| d.name == *name) {
                DevicePlan::Named(name.clone())
            } else {
                DevicePlan::Default {
                    because_missing: Some(name.clone()),
                }
            }
        }
    }
}

/// The selection a stream opened under `plan` actually captures from.
/// A fallback counts as [`DeviceSelection::SystemDefault`], so the
/// next press re-resolves the (possibly re-attached) preferred device
/// instead of latching onto the fallback stream forever.
fn selection_of_plan(plan: &DevicePlan) -> DeviceSelection {
    match plan {
        DevicePlan::Default { .. } => DeviceSelection::SystemDefault,
        DevicePlan::Named(name) => DeviceSelection::Preferred(name.clone()),
    }
}

/// Open the input device a recording wants (the impure half of
/// [`resolve_device_plan`]): the named preferred device when the plan
/// says so, else the system default — with a warning when that is a
/// fallback from a preferred device missing at open time.
fn open_input_device(host: &cpal::Host, plan: &DevicePlan) -> Option<cpal::Device> {
    match plan {
        DevicePlan::Default { because_missing } => {
            if let Some(missing) = because_missing {
                tracing::warn!(
                    "Preferred input device {:?} not found — falling back \
                     to the system default",
                    missing
                );
            }
            host.default_input_device()
        }
        DevicePlan::Named(name) => host.input_devices().ok().and_then(|mut devices| {
            devices.find(|d| d.name().ok().as_deref() == Some(name.as_str()))
        }),
    }
}

/// What to do about a possibly-failed stream mid-recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MicVerdict {
    /// Healthy or merely glitched (an xrun that kept delivering) —
    /// keep capturing.
    Keep,
    /// Errored AND silent for the stall grace period — the device is
    /// gone; reopen.
    Reopen,
}

/// An error alone is not fatal (xruns recover and keep delivering
/// data); a stall alone is not actionable (no error to attribute it
/// to). Both together mean the stream is dead.
pub(crate) fn mid_recording_verdict(stream_errored: bool, stalled_ms: u64) -> MicVerdict {
    if stream_errored && stalled_ms >= MID_RECORDING_STALL_MS {
        MicVerdict::Reopen
    } else {
        MicVerdict::Keep
    }
}

// ── Stream construction ───────────────────────────────────────────────

/// Preferred capture period in frames. A press-to-record waits out the
/// *remaining* period of the running stream before `first_audio`, and
/// the host default here is ~2048 frames (~43 ms @ 44.1 kHz) — which
/// alone would dwarf the 15 ms warm-path target. 256 frames is
/// ~2.7–5.8 ms per block (measured 2.65 ms mean on PipeWire 44.1 kHz)
/// at a trivial wakeup rate (~170/s while parked).
const PREFERRED_PERIOD_FRAMES: u32 = 256;

/// Build an input stream whose callbacks convert to mono and append
/// into the shared ring (see [`push_block`]). Tries the small period
/// first; a device that rejects it falls back to the host default
/// (correctness over latency — the default is exactly what the
/// pre-vew code used).
fn build_ring_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    state: Arc<Mutex<WarmState>>,
) -> anyhow::Result<cpal::Stream> {
    let mut small: cpal::StreamConfig = supported.clone().into();
    small.buffer_size = cpal::BufferSize::Fixed(PREFERRED_PERIOD_FRAMES);
    match build_ring_stream_with_config(device, supported, &small, Arc::clone(&state)) {
        Ok(stream) => Ok(stream),
        Err(e) => {
            tracing::warn!(
                "Small capture period ({} frames) rejected ({}) — using the device default",
                PREFERRED_PERIOD_FRAMES,
                e
            );
            let default: cpal::StreamConfig = supported.clone().into();
            build_ring_stream_with_config(device, supported, &default, state)
        }
    }
}

fn build_ring_stream_with_config(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<WarmState>>,
) -> anyhow::Result<cpal::Stream> {
    let channels = supported.channels() as usize;

    match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            let data_state = Arc::clone(&state);
            let err_state = Arc::clone(&state);
            device
                .build_input_stream(
                    config,
                    move |data: &[f32], _| {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                            .collect();
                        push_block(&data_state, &mono);
                    },
                    move |err| record_stream_error(&err_state, &err),
                    None,
                )
                .map_err(Into::into)
        }
        cpal::SampleFormat::I16 => {
            let data_state = Arc::clone(&state);
            let err_state = Arc::clone(&state);
            device
                .build_input_stream(
                    config,
                    move |data: &[i16], _| {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| {
                                frame
                                    .iter()
                                    .map(|&s| s as f32 / i16::MAX as f32)
                                    .sum::<f32>()
                                    / channels as f32
                            })
                            .collect();
                        push_block(&data_state, &mono);
                    },
                    move |err| record_stream_error(&err_state, &err),
                    None,
                )
                .map_err(Into::into)
        }
        _ => anyhow::bail!("Unsupported sample format"),
    }
}

/// Data-callback core: append a mono block and decide whether this is
/// the block that crosses the armed `first_audio` threshold. The mark
/// itself is emitted outside the lock (it does file IO).
fn push_block(state: &Mutex<WarmState>, mono: &[f32]) {
    let crossed = {
        let mut st = state.lock();
        st.ring.push(mono);
        let crossed = !st.first_audio_marked
            && st.first_audio_at != DISARMED
            && st.ring.written() > st.first_audio_at;
        if crossed {
            st.first_audio_marked = true;
        }
        crossed
    };
    if crossed {
        timing::mark("first_audio");
    }
}

/// Error-callback core: log and count. The count is what refuses
/// reuse at the next press and (with a stall) triggers the
/// mid-recording reopen.
fn record_stream_error(state: &Mutex<WarmState>, err: &cpal::StreamError) {
    tracing::error!("Audio error: {}", err);
    state.lock().error_count += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RingBuffer ────────────────────────────────────────────────────

    /// The basic contract: pushed samples come back in capture order,
    /// addressed by absolute index.
    #[test]
    fn ring_push_and_take_roundtrip() {
        let mut ring = RingBuffer {
            data: vec![0.0; 8],
            written: 0,
        };
        ring.push(&[1.0, 2.0, 3.0]);
        ring.push(&[4.0, 5.0]);

        assert_eq!(ring.written(), 5);
        assert_eq!(ring.take_range(0..5), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(ring.take_range(2..4), vec![3.0, 4.0]);
    }

    /// Wraparound must not scramble order: samples crossing the end of
    /// the backing storage still read back oldest-first.
    #[test]
    fn ring_wraps_and_preserves_order() {
        let mut ring = RingBuffer {
            data: vec![0.0; 4],
            written: 0,
        };
        for i in 0..10u64 {
            ring.push(&[i as f32]);
        }
        assert_eq!(ring.written(), 10);
        // Oldest survivor is 10 - 4 = 6.
        assert_eq!(ring.take_range(6..10), vec![6.0, 7.0, 8.0, 9.0]);
        // A range spanning the wrap seam.
        assert_eq!(ring.take_range(7..10), vec![7.0, 8.0, 9.0]);
    }

    /// A block larger than the whole ring keeps only its newest end —
    /// the ring never grows unbounded, whatever the callback delivers.
    #[test]
    fn ring_oversized_block_keeps_only_the_tail() {
        let mut ring = RingBuffer {
            data: vec![0.0; 4],
            written: 0,
        };
        let chunk: Vec<f32> = (0..10).map(|i| i as f32).collect();
        ring.push(&chunk);

        assert_eq!(ring.written(), 10);
        assert_eq!(ring.take_range(6..10), vec![6.0, 7.0, 8.0, 9.0]);
    }

    /// Reads older than the ring depth are clamped to the oldest
    /// survivor — the drain loses early audio but never panics or
    /// returns garbage.
    #[test]
    fn ring_clamps_reads_older_than_capacity() {
        let mut ring = RingBuffer {
            data: vec![0.0; 4],
            written: 0,
        };
        for i in 0..10u64 {
            ring.push(&[i as f32]);
        }

        assert_eq!(ring.take_range(0..10), vec![6.0, 7.0, 8.0, 9.0]);
        // Fully-evicted range → empty.
        assert!(ring.take_range(0..3).is_empty());
    }

    /// Resizing for a new sample rate must keep the absolute counter:
    /// live drain cursors and press offsets stay valid across the swap
    /// (already-drained audio lives in the recording buffer, not the
    /// ring — at most one drain tick of undrained samples is zeroed).
    #[test]
    fn ring_resize_preserves_written_counter() {
        let mut ring = RingBuffer {
            data: vec![0.0; 4],
            written: 0,
        };
        ring.push(&[1.0, 2.0, 3.0]);
        assert_eq!(ring.written(), 3);

        ring.ensure_capacity_for(48_000, 5.0);

        assert_eq!(ring.written(), 3, "absolute offsets must survive resize");
        ring.push(&[4.0]);
        assert_eq!(ring.take_range(3..4), vec![4.0]);
    }

    // ── drain decision ────────────────────────────────────────────────

    /// A cursor inside the ring drains the exact fresh span with no
    /// overrun flag; one behind the ring depth is clamped and flagged.
    #[test]
    fn drain_range_flags_overrun_only_when_behind() {
        let (range, overran) = clamped_drain_range(90, 100, 50);
        assert_eq!(range, 90..100);
        assert!(!overran);

        let (range, overran) = clamped_drain_range(10, 100, 50);
        assert_eq!(range, 50..100, "clamped to the oldest survivor");
        assert!(overran);
    }

    // ── first_audio threshold ─────────────────────────────────────────

    fn armed_state(cap: usize, threshold: u64) -> Mutex<WarmState> {
        let mut st = WarmState::new();
        st.ring = RingBuffer {
            data: vec![0.0; cap],
            written: 0,
        };
        st.first_audio_at = threshold;
        Mutex::new(st)
    }

    /// The mark fires exactly once, on the first block that crosses
    /// the armed press offset — and never while disarmed.
    #[test]
    fn first_audio_fires_once_on_the_crossing_block() {
        let st = armed_state(64, DISARMED);

        push_block(&st, &[0.1, 0.2]); // parked capture, not armed
        assert!(!st.lock().first_audio_marked, "disarmed must not mark");

        // A press arms the threshold at the current write position.
        let threshold = st.lock().ring.written();
        st.lock().first_audio_at = threshold;

        push_block(&st, &[0.3]);
        assert!(st.lock().first_audio_marked, "crossing block must mark");

        push_block(&st, &[0.4, 0.5]);
        assert!(st.lock().first_audio_marked, "marked stays latched");
    }

    /// A block that ends exactly at the threshold contains no
    /// post-offset sample and must NOT mark; the next one does. This
    /// is what keeps `first_audio` the first sample *after* the press
    /// offset rather than "whenever the callback ran".
    #[test]
    fn first_audio_requires_strictly_post_offset_samples() {
        let st = armed_state(64, DISARMED);
        push_block(&st, &[0.1, 0.2]);

        // Arm at a FUTURE position: 4 samples from now.
        let mut st_lock = st.lock();
        let future = st_lock.ring.written() + 4;
        st_lock.first_audio_at = future;
        drop(st_lock);

        push_block(&st, &[0.3, 0.4, 0.5]); // written = 5 ≤ 6: no post-offset sample
        assert!(!st.lock().first_audio_marked);

        push_block(&st, &[0.6]); // written = 6: ends exactly at the threshold
        assert!(!st.lock().first_audio_marked);

        push_block(&st, &[0.7]); // written = 7 > 6: crosses
        assert!(st.lock().first_audio_marked);
    }

    // ── decisions ─────────────────────────────────────────────────────

    /// Park only while the wanted device still matches the captured
    /// one. System-default mode: the default must match (any mismatch
    /// or a missing default releases). Preferred mode: the parked
    /// stream must BE that device — the default is irrelevant.
    #[test]
    fn park_decision_requires_matching_wanted_device() {
        let a = DeviceIdentity {
            name: "Mic A".into(),
            sample_rate: 48_000,
            channels: 2,
        };
        let a2 = a.clone();
        let b = DeviceIdentity {
            name: "Mic B".into(),
            sample_rate: 48_000,
            channels: 2,
        };
        // Same device moved to a different default rate also counts as
        // changed: the parked stream's config no longer matches.
        let a_other_rate = DeviceIdentity {
            name: "Mic A".into(),
            sample_rate: 44_100,
            channels: 2,
        };
        let prefer_a = DeviceSelection::Preferred("Mic A".into());

        // System-default mode: unchanged semantics.
        assert!(park_decision(
            Some(&a),
            &DeviceSelection::SystemDefault,
            Some(&a2)
        ));
        assert!(!park_decision(
            Some(&a),
            &DeviceSelection::SystemDefault,
            Some(&b)
        ));
        assert!(!park_decision(
            Some(&a),
            &DeviceSelection::SystemDefault,
            Some(&a_other_rate)
        ));
        assert!(
            !park_decision(Some(&a), &DeviceSelection::SystemDefault, None),
            "no default → release"
        );
        assert!(
            !park_decision(None, &DeviceSelection::SystemDefault, Some(&a2)),
            "no identity → release"
        );

        // Preferred mode: the parked stream must capture from the
        // preferred device; a default switch (even to a different
        // device, or no default at all) must NOT release it.
        assert!(park_decision(Some(&a), &prefer_a, Some(&b)));
        assert!(park_decision(Some(&a), &prefer_a, None));
        assert!(!park_decision(
            Some(&a),
            &DeviceSelection::Preferred("Mic B".into()),
            Some(&a2)
        ));
    }

    // ── preferred device (canario-1hq.2) ─────────────────────────────

    /// The process-wide store: Some(non-blank) stores (trimmed),
    /// None/blank clears — same posture as the sidecar's credential
    /// store tests.
    #[test]
    fn preferred_device_store_round_trip_and_clearing() {
        let _guard = PREFERRED_TEST_LOCK.lock();
        set_preferred_device(None);
        assert!(preferred_device().is_none());

        set_preferred_device(Some("USB Mic".into()));
        assert_eq!(preferred_device().as_deref(), Some("USB Mic"));

        // Blank counts as absent (system default), not a stored name.
        set_preferred_device(Some("   ".into()));
        assert!(preferred_device().is_none());

        // Whitespace is trimmed, not stored verbatim.
        set_preferred_device(Some("  Trimmed  ".into()));
        assert_eq!(preferred_device().as_deref(), Some("Trimmed"));

        set_preferred_device(None);
        assert!(preferred_device().is_none());
    }

    /// The preference maps onto the recording's wanted selection:
    /// absent/blank → system default, a name → that device.
    #[test]
    fn current_selection_maps_preference_to_selection() {
        let _guard = PREFERRED_TEST_LOCK.lock();
        set_preferred_device(None);
        assert_eq!(current_selection(), DeviceSelection::SystemDefault);

        set_preferred_device(Some("Mic B".into()));
        assert_eq!(
            current_selection(),
            DeviceSelection::Preferred("Mic B".into())
        );

        set_preferred_device(None);
    }

    /// A healthy parked stream is reused ONLY when it was opened for
    /// the selection now wanted: an errored stream — or one opened for
    /// a different device (the preference changed while parked, in
    /// either direction) — is released and the wanted device opened
    /// fresh.
    #[test]
    fn begin_verdict_requires_health_and_matching_selection() {
        let a = DeviceSelection::Preferred("Mic A".into());
        let a2 = a.clone();
        let b = DeviceSelection::Preferred("Mic B".into());
        let sys = DeviceSelection::SystemDefault;

        // Healthy + matching → reuse.
        assert_eq!(
            begin_verdict(true, false, Some(&a), &a2),
            BeginVerdict::Reuse
        );
        assert_eq!(
            begin_verdict(true, false, Some(&sys), &sys),
            BeginVerdict::Reuse
        );
        // Errored, or no stream, or no recorded selection → release.
        assert_eq!(
            begin_verdict(true, true, Some(&a), &a2),
            BeginVerdict::Release
        );
        assert_eq!(
            begin_verdict(false, false, Some(&a), &a2),
            BeginVerdict::Release
        );
        assert_eq!(begin_verdict(true, false, None, &a2), BeginVerdict::Release);
        // Selection mismatch in every direction → release.
        assert_eq!(
            begin_verdict(true, false, Some(&a), &b),
            BeginVerdict::Release
        );
        assert_eq!(
            begin_verdict(true, false, Some(&a), &sys),
            BeginVerdict::Release
        );
        assert_eq!(
            begin_verdict(true, false, Some(&sys), &a),
            BeginVerdict::Release
        );
    }

    /// The preferred device is opened by name when the enumeration
    /// lists it; a preferred device missing at open time falls back
    /// to the system default, flagged so the caller can warn.
    #[test]
    fn resolve_device_plan_prefers_named_device_with_default_fallback() {
        let devices = vec![
            MicDevice {
                name: "Mic A".into(),
            },
            MicDevice {
                name: "Mic B".into(),
            },
        ];

        // System default wanted → the default, no fallback flag.
        assert_eq!(
            resolve_device_plan(&DeviceSelection::SystemDefault, &devices),
            DevicePlan::Default {
                because_missing: None
            }
        );
        // Preferred + enumerated → open it by name.
        assert_eq!(
            resolve_device_plan(&DeviceSelection::Preferred("Mic B".into()), &devices),
            DevicePlan::Named("Mic B".into())
        );
        // Preferred + missing (unplugged, typo, empty host) → fall
        // back to the default, flagged for the warning.
        assert_eq!(
            resolve_device_plan(&DeviceSelection::Preferred("Gone".into()), &devices),
            DevicePlan::Default {
                because_missing: Some("Gone".into())
            }
        );
        assert_eq!(
            resolve_device_plan(&DeviceSelection::Preferred("Gone".into()), &[]),
            DevicePlan::Default {
                because_missing: Some("Gone".into())
            }
        );
    }

    /// A fallback open counts as SystemDefault for reuse purposes, so
    /// the next press retries the (possibly re-attached) preferred
    /// device instead of latching onto the fallback stream.
    #[test]
    fn selection_of_plan_records_fallback_as_system_default() {
        assert_eq!(
            selection_of_plan(&DevicePlan::Default {
                because_missing: None
            }),
            DeviceSelection::SystemDefault
        );
        assert_eq!(
            selection_of_plan(&DevicePlan::Default {
                because_missing: Some("Gone".into())
            }),
            DeviceSelection::SystemDefault
        );
        assert_eq!(
            selection_of_plan(&DevicePlan::Named("Mic B".into())),
            DeviceSelection::Preferred("Mic B".into())
        );
    }

    /// Enumeration dedupes by name (first occurrence wins) and drops
    /// unreadable/blank names — the picker's list is clean whatever
    /// the host reports.
    #[test]
    fn deduped_devices_keeps_first_occurrence_and_drops_unusable_names() {
        let names = vec![
            "Mic A".to_string(),
            String::new(),
            "Mic B".to_string(),
            "Mic A".to_string(),
            "Mic B".to_string(),
        ];
        assert_eq!(
            deduped_devices(names.into_iter()),
            vec![
                MicDevice {
                    name: "Mic A".into()
                },
                MicDevice {
                    name: "Mic B".into()
                },
            ]
        );
        assert!(deduped_devices(std::iter::empty()).is_empty());
    }

    /// Error alone (recoverable xrun) and stall alone (no error to
    /// attribute) both keep the stream; only both together reopen —
    /// and only once the stall passed the grace period.
    #[test]
    fn mid_recording_verdict_needs_error_and_stall() {
        assert_eq!(mid_recording_verdict(true, 0), MicVerdict::Keep);
        assert_eq!(
            mid_recording_verdict(true, MID_RECORDING_STALL_MS - 1),
            MicVerdict::Keep
        );
        assert_eq!(mid_recording_verdict(false, 10_000), MicVerdict::Keep);
        assert_eq!(
            mid_recording_verdict(true, MID_RECORDING_STALL_MS),
            MicVerdict::Reopen
        );
    }

    // ── session / owner-loop lifecycle (canario-2z0) ──────────────────

    fn test_session(mic: &WarmMic) -> MicSession {
        MicSession {
            shared: Arc::clone(&mic.shared),
            sample_rate: 16_000,
            start_offset: 0,
            errors_at_begin: 0,
            finished: false,
            owner: None,
        }
    }

    /// Run a real owner loop against `mic`, keeping the command sender
    /// alive; returns it so the caller can stop the thread by dropping.
    fn spawn_owner_loop(mic: &WarmMic) -> std::sync::mpsc::Sender<OwnerCmd> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mic = WarmMic {
            shared: Arc::clone(&mic.shared),
        };
        std::thread::spawn(move || owner_loop(rx, mic));
        tx
    }

    fn force_parked(mic: &WarmMic, window: Duration) {
        mic.shared.state.lock().phase = Phase::Parked {
            deadline: Instant::now() + window,
        };
    }

    /// The owner loop fully releases a parked stream once the idle
    /// window lapses — this is the "no leaked mic indicator" guarantee.
    /// (The idle wait IS the watch: `recv_timeout` on the deadline in
    /// `Phase::Parked` replaces the old reaper thread.)
    #[test]
    fn owner_loop_releases_after_the_idle_window() {
        let mic = WarmMic::new(Duration::from_millis(50), 1.0);
        force_parked(&mic, Duration::from_millis(50));
        let _keep_alive = spawn_owner_loop(&mic);

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if matches!(mic.shared.state.lock().phase, Phase::Released) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "owner loop did not release within 2 s"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// While a recording owns the stream the owner loop must NOT
    /// release it: `Phase::Recording` means the idle wait has no
    /// deadline to watch, so a parked-window that elapsed "during" the
    /// recording can't fire into it.
    #[test]
    fn owner_loop_holds_while_a_recording_owns_the_stream() {
        let mic = WarmMic::new(Duration::from_millis(40), 1.0);
        let _keep_alive = spawn_owner_loop(&mic);
        mic.shared.state.lock().phase = Phase::Recording;

        std::thread::sleep(Duration::from_millis(300));
        assert!(
            matches!(mic.shared.state.lock().phase, Phase::Recording),
            "owner loop must not release while a recording is active"
        );
    }

    /// finish() with no stream must leave Released (no park), and be
    /// idempotent across the explicit call and Drop. Local-mode
    /// sessions (owner: None) do the disarm/release bookkeeping the
    /// owner would have — same observable end state.
    #[test]
    fn session_finish_without_stream_releases_and_is_idempotent() {
        let mic = WarmMic::new(Duration::from_secs(60), 1.0);
        let mut session = test_session(&mic);

        session.finish();
        session.finish(); // idempotent
        drop(session); // Drop runs the net — must be a no-op now

        let st = mic.shared.state.lock();
        assert!(matches!(st.phase, Phase::Released));
        assert_eq!(st.first_audio_at, DISARMED, "finish disarms first_audio");
    }
}

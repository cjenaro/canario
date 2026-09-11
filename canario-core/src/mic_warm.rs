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
//! released by a reaper thread (`mic_idle_released` mark). Rapid
//! dictations stay warm; the indicator is dark at startup and at most
//! 60 s after the last dictation.
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
//! * At park time the default input device is re-queried: if it no
//!   longer matches the captured device (default-source switch while
//!   parked), the stream is released instead, so a stale device is
//!   captured for at most one dictation.
//!
//! # Timing marks
//!
//! `mic_warm_reused` (press reused the parked stream),
//! `mic_idle_released` (reaper released the mic after the idle
//! window); the cold path keeps `mic_device_opened`,
//! `mic_stream_started` and `first_audio` with their original
//! meanings.

use std::ops::Range;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::{Condvar, Mutex};

use crate::timing;

/// How long the stream stays parked after the last recording before
/// the reaper releases it (mic indicator off, device closed).
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
    /// Stream alive but idle; the reaper releases it at `deadline`.
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

/// Identity of the current default input device, if one exists.
fn default_input_identity() -> Option<DeviceIdentity> {
    let device = cpal::default_host().default_input_device()?;
    let supported = device.default_input_config().ok()?;
    Some(DeviceIdentity::of(&device, &supported))
}

/// Everything guarded by the state mutex. The data/error callbacks,
/// the recording thread and the reaper all take this lock briefly —
/// but a [`cpal::Stream`] is never *dropped* while it is held, because
/// dropping joins the audio thread, which may itself be waiting on
/// this lock inside its callback.
struct WarmState {
    ring: RingBuffer,
    stream: Option<cpal::Stream>,
    stream_identity: Option<DeviceIdentity>,
    phase: Phase,
    /// Cumulative error-callback count for the live stream (reset when
    /// a stream is stored).
    error_count: u64,
    /// `written` value whose crossing by a data block marks
    /// `first_audio`; `DISARMED` while parked/stopped.
    first_audio_at: u64,
    first_audio_marked: bool,
    reaper_alive: bool,
}

impl WarmState {
    fn new() -> Self {
        Self {
            ring: RingBuffer {
                data: Vec::new(),
                written: 0,
            },
            stream: None,
            stream_identity: None,
            phase: Phase::Released,
            error_count: 0,
            first_audio_at: DISARMED,
            first_audio_marked: false,
            reaper_alive: false,
        }
    }
}

/// State plus the condvar the reaper waits on, shared with the
/// recording session.
struct SharedMic {
    state: Arc<Mutex<WarmState>>,
    cond: Condvar,
    window: Duration,
    ring_secs: f64,
}

impl SharedMic {
    /// The state mutex as a fresh `Arc` handle, for capture callbacks.
    fn state_arc(self: &Arc<Self>) -> Arc<Mutex<WarmState>> {
        Arc::clone(&self.state)
    }
}

/// The warm-mic machinery. One instance lives in the process global
/// ([`global`]); tests construct their own.
struct WarmMic {
    shared: Arc<SharedMic>,
}

impl WarmMic {
    fn new(window: Duration, ring_secs: f64) -> Self {
        Self {
            shared: Arc::new(SharedMic {
                state: Arc::new(Mutex::new(WarmState::new())),
                cond: Condvar::new(),
                window,
                ring_secs,
            }),
        }
    }
}

/// Process-global warm mic.
fn global() -> &'static WarmMic {
    static MIC: LazyLock<WarmMic> = LazyLock::new(|| WarmMic::new(PARK_WINDOW, RING_SECS));
    &MIC
}

/// Acquire the mic for a recording: reuse the parked stream when
/// healthy (marks `mic_warm_reused`), else open a fresh one inline
/// (marks `mic_device_opened` + `mic_stream_started`, the pre-vew
/// path). Arms `first_audio` at the current write position either
/// way.
pub(crate) fn begin_recording() -> anyhow::Result<MicSession> {
    global().begin_recording()
}

impl WarmMic {
    fn begin_recording(&self) -> anyhow::Result<MicSession> {
        // Fast path — a healthy parked stream: arm and go (µs). The
        // arm runs under the state lock so a concurrent data block
        // cannot slip between the threshold store and the flag reset.
        let reused = {
            let mut st = self.shared.state.lock();
            if st.stream.is_some() && st.error_count == 0 {
                let offset = st.ring.written();
                st.first_audio_marked = false;
                st.first_audio_at = offset;
                st.phase = Phase::Recording;
                self.shared.cond.notify_all(); // reaper: re-read phase
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
            return Ok(MicSession {
                shared: Arc::clone(&self.shared),
                sample_rate,
                start_offset: offset,
                errors_at_begin: 0,
                finished: false,
            });
        }

        // An errored parked stream must not be reused. Drop it OUTSIDE
        // the lock first (cpal joins its audio thread on drop).
        let failed = {
            let mut st = self.shared.state.lock();
            st.stream_identity = None;
            if st.stream.is_some() {
                st.phase = Phase::Released;
            }
            st.stream.take()
        };
        if failed.is_some() {
            tracing::info!("Parked mic stream was unhealthy — reopening input device");
        }
        drop(failed);

        // Cold open, paid inline. Holding the state lock across the
        // open is fine: no stream exists, so no data callback can
        // contend (the only other waiter is the reaper condvar, which
        // releases the lock while waiting).
        let mut st = self.shared.state.lock();
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
        let supported = device.default_input_config()?;
        let identity = DeviceIdentity::of(&device, &supported);
        let sample_rate = supported.sample_rate().0;
        tracing::info!("Recording from '{}' at {}Hz", identity.name, sample_rate);
        timing::mark("mic_device_opened");

        let stream = build_ring_stream(&device, &supported, self.shared.state_arc())?;
        st.ring
            .ensure_capacity_for(sample_rate, self.shared.ring_secs);
        // Arm before play: the first post-play block must be the one
        // that marks `first_audio` (pre-vew semantics).
        let offset = st.ring.written();
        st.first_audio_marked = false;
        st.first_audio_at = offset;
        st.error_count = 0;
        st.stream_identity = Some(identity);
        st.phase = Phase::Recording;
        st.stream = Some(stream);
        let played = st.stream.as_ref().expect("stored above").play();
        drop(st);
        if let Err(e) = played {
            let failed = self.shared.state.lock().stream.take();
            drop(failed);
            return Err(e.into());
        }
        timing::mark("mic_stream_started");

        Ok(MicSession {
            shared: Arc::clone(&self.shared),
            sample_rate,
            start_offset: offset,
            errors_at_begin: 0,
            finished: false,
        })
    }
}

// ── Recording session ─────────────────────────────────────────────────

/// A recording's handle on the warm mic: the press offset, the drain
/// cursor API, mid-recording error recovery, and the park-on-finish
/// transition. `finish` is idempotent; `Drop` finishes too, so a
/// panic between begin and finish still parks (and the reaper still
/// releases after the window).
pub(crate) struct MicSession {
    shared: Arc<SharedMic>,
    sample_rate: u32,
    start_offset: u64,
    errors_at_begin: u64,
    finished: bool,
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

    /// Mid-recording fallback: the stream failed. Drop it and open a
    /// fresh one continuing into the same ring — absolute offsets are
    /// preserved, so the drain cursor and any unmarked `first_audio`
    /// threshold stay valid. Costs a short audio gap (the reopen).
    pub(crate) fn reopen_after_error(&self) -> anyhow::Result<()> {
        tracing::warn!(
            "Mic stream failed mid-recording — reopening the input device \
             (short gap in captured audio)"
        );
        let failed = self.shared.state.lock().stream.take();
        drop(failed); // outside the lock: cpal joins the audio thread

        let mut st = self.shared.state.lock();
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device found"))?;
        let supported = device.default_input_config()?;
        let identity = DeviceIdentity::of(&device, &supported);
        let stream = build_ring_stream(&device, &supported, self.shared.state_arc())?;
        st.ring
            .ensure_capacity_for(identity.sample_rate, self.shared.ring_secs);
        st.stream_identity = Some(identity);
        st.error_count = 0;
        st.stream = Some(stream);
        let played = st.stream.as_ref().expect("stored above").play();
        drop(st);
        if let Err(e) = played {
            let failed = self.shared.state.lock().stream.take();
            drop(failed);
            return Err(e.into());
        }
        Ok(())
    }

    /// End of recording: disarm `first_audio`, then either park for
    /// the idle window (arming the reaper) or release now when the
    /// default device no longer matches the captured one.
    pub(crate) fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;

        let mut released: Option<cpal::Stream> = None;
        let keep = {
            let mut st = self.shared.state.lock();
            st.first_audio_at = DISARMED;
            let keep = st.stream.is_some()
                && park_decision(
                    st.stream_identity.as_ref(),
                    default_input_identity().as_ref(),
                );
            if keep {
                st.phase = Phase::Parked {
                    deadline: Instant::now() + self.shared.window,
                };
            } else {
                st.stream_identity = None;
                st.phase = Phase::Released;
                released = st.stream.take();
            }
            keep
        };
        drop(released); // outside the lock: cpal joins the audio thread

        if keep && !ensure_reaper(&self.shared) {
            // No watchdog possible — release now rather than risk a
            // permanently lit mic indicator.
            tracing::warn!("Warm mic reaper unspawnable — releasing stream immediately");
            let failed = {
                let mut st = self.shared.state.lock();
                st.stream_identity = None;
                st.phase = Phase::Released;
                st.stream.take()
            };
            drop(failed);
        }
    }
}

impl Drop for MicSession {
    fn drop(&mut self) {
        self.finish();
    }
}

// ── Decisions (pure — unit-testable seams) ────────────────────────────

/// Should the stream stay parked after a recording? Only when the
/// default input device still matches the captured one — a hotplug or
/// default-source switch must not capture from a stale device forever.
fn park_decision(
    parked: Option<&DeviceIdentity>,
    current_default: Option<&DeviceIdentity>,
) -> bool {
    match (parked, current_default) {
        (Some(p), Some(c)) => p == c,
        _ => false,
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

// ── Reaper ────────────────────────────────────────────────────────────

/// Make sure a reaper thread is watching the park deadline (or re-read
/// the fresh deadline if it already is). Returns `false` when the
/// thread could not be spawned.
fn ensure_reaper(shared: &Arc<SharedMic>) -> bool {
    let mut st = shared.state.lock();
    if st.reaper_alive {
        shared.cond.notify_all();
        return true;
    }
    st.reaper_alive = true;
    drop(st);

    let spawned = std::thread::Builder::new()
        .name("mic-warm-reaper".to_string())
        .spawn({
            let shared = Arc::clone(shared);
            move || reaper_loop(shared)
        });
    match spawned {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("Could not spawn mic-warm reaper: {}", e);
            shared.state.lock().reaper_alive = false;
            false
        }
    }
}

/// Watch the parked stream and fully release it when the idle window
/// lapses. Exits when there is nothing left to watch.
fn reaper_loop(shared: Arc<SharedMic>) {
    let mut st = shared.state.lock();
    loop {
        match st.phase {
            Phase::Released => {
                st.reaper_alive = false;
                return;
            }
            Phase::Recording => {
                // A recording owns the stream; wait for the next
                // transition (finish notifies).
                shared.cond.wait(&mut st);
            }
            Phase::Parked { deadline } => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let stream = st.stream.take();
                    st.stream_identity = None;
                    st.phase = Phase::Released;
                    st.reaper_alive = false;
                    drop(st); // release the lock before joining the audio thread
                    drop(stream);
                    timing::mark("mic_idle_released");
                    tracing::info!(
                        "Warm mic idle window elapsed — capture stream released (mic off)"
                    );
                    return;
                }
                shared.cond.wait_for(&mut st, remaining);
            }
        }
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

    /// Park only while the default device still matches; any mismatch
    /// or a missing default releases.
    #[test]
    fn park_decision_requires_matching_default_device() {
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

        assert!(park_decision(Some(&a), Some(&a2)));
        assert!(!park_decision(Some(&a), Some(&b)));
        assert!(!park_decision(Some(&a), Some(&a_other_rate)));
        assert!(!park_decision(Some(&a), None), "no default → release");
        assert!(!park_decision(None, Some(&a2)), "no identity → release");
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

    // ── session / reaper lifecycle ────────────────────────────────────

    fn test_session(mic: &WarmMic) -> MicSession {
        MicSession {
            shared: Arc::clone(&mic.shared),
            sample_rate: 16_000,
            start_offset: 0,
            errors_at_begin: 0,
            finished: false,
        }
    }

    fn force_parked(mic: &WarmMic, window: Duration) {
        mic.shared.state.lock().phase = Phase::Parked {
            deadline: Instant::now() + window,
        };
    }

    /// The reaper fully releases a parked stream once the idle window
    /// lapses — this is the "no leaked mic indicator" guarantee.
    #[test]
    fn reaper_releases_after_the_idle_window() {
        let mic = WarmMic::new(Duration::from_millis(50), 1.0);
        force_parked(&mic, Duration::from_millis(50));

        assert!(ensure_reaper(&mic.shared));

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let released = {
                let st = mic.shared.state.lock();
                matches!(st.phase, Phase::Released) && !st.reaper_alive
            };
            if released {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "reaper did not release within 2 s"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A recording that begins inside the window must cancel the
    /// pending release: begin notifies the reaper, which then waits
    /// instead of dropping the stream mid-recording.
    #[test]
    fn reaper_holds_while_a_recording_owns_the_stream() {
        let mic = WarmMic::new(Duration::from_millis(40), 1.0);
        force_parked(&mic, Duration::from_millis(40));
        assert!(ensure_reaper(&mic.shared));

        // Simulate begin_recording's phase transition + notify.
        {
            let mut st = mic.shared.state.lock();
            st.phase = Phase::Recording;
            mic.shared.cond.notify_all();
        }

        std::thread::sleep(Duration::from_millis(300));
        assert!(
            matches!(mic.shared.state.lock().phase, Phase::Recording),
            "reaper must not release while a recording is active"
        );
    }

    /// finish() with no stream must leave Released (no park, no
    /// reaper), and be idempotent across the explicit call and Drop.
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
        assert!(!st.reaper_alive);
    }
}

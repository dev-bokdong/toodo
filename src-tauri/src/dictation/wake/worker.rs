//! The wake word worker of the macOS and Windows apps: a thread that downloads and loads the
//! recognizer, takes the microphone from the platform ([`platform`]), resamples it to 16 kHz,
//! and feeds the detector in 100 ms chunks. It opens the microphone again when audio stops
//! coming or the platform reports the stream broken (the computer slept, the input device
//! changed), and waits for the microphone permission to come back when it was taken away.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sherpa_onnx::LinearResampler;
use tauri::AppHandle;
use tauri_specta::Event;

#[cfg(target_os = "macos")]
use super::macos as platform;
#[cfg(windows)]
use super::windows as platform;

use super::detector::{Detector, Event as Heard, Holds, SAMPLE_RATE};
use super::engine::Sherpa;
use super::{WakeWordChanged, WakeWordError, WakeWordErrorKind, WakeWordHeard, WakeWordStatus};
use super::{download, model};
use platform::Audio;

const CHUNK: usize = SAMPLE_RATE as usize / 10;
/// Buffers of 10 to 25 ms each: a few seconds of slack for a slow decode.
const QUEUE: usize = 256;
/// Without audio for this long, the microphone is taken to have stopped.
const SILENT_DEVICE: Duration = Duration::from_secs(3);
/// How often a stopped microphone, or a missing permission, is tried again, and how often
/// the permission is checked while listening.
const RETRY: Duration = Duration::from_secs(5);
/// The directory in the app data directory that holds the model.
const MODEL_DIRECTORY: &str = "wake-word-model";

/// What the commands tell the worker; it checks these between chunks.
#[derive(Default)]
struct Control {
    stop: AtomicBool,
    cancel: AtomicBool,
    speaking: AtomicBool,
    dictating: AtomicBool,
}

struct Worker {
    id: u32,
    control: Arc<Control>,
    thread: JoinHandle<()>,
}

struct Shared {
    app: AppHandle,
    data_dir: PathBuf,
    enabled: bool,
    status: WakeWordStatus,
    worker: Option<Worker>,
    speaking: bool,
    dictating: bool,
}

static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
static WORKERS: AtomicU32 = AtomicU32::new(0);
static HEARD: AtomicU32 = AtomicU32::new(0);

fn with_shared<T>(f: impl FnOnce(&mut Shared) -> T) -> Option<T> {
    SHARED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
        .map(f)
}

pub fn init(app: &AppHandle, data_dir: PathBuf) {
    *SHARED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Shared {
        app: app.clone(),
        data_dir,
        enabled: false,
        status: WakeWordStatus::Off,
        worker: None,
        speaking: false,
        dictating: false,
    });
}

pub fn data_dir() -> Option<PathBuf> {
    with_shared(|shared| shared.data_dir.clone())
}

pub fn current() -> WakeWordChanged {
    with_shared(|shared| WakeWordChanged {
        enabled: shared.enabled,
        status: shared.status.clone(),
    })
    .unwrap_or(WakeWordChanged {
        enabled: false,
        status: WakeWordStatus::Off,
    })
}

/// Sets the status if `worker` is still the running one (any worker for `None`), and
/// tells the frontend and the tray icon.
fn set_status(worker: Option<u32>, status: WakeWordStatus) {
    let changed = with_shared(|shared| {
        let current = shared.worker.as_ref().map(|w| w.id);
        if worker.is_some() && worker != current || shared.status == status {
            return None;
        }
        shared.status = status.clone();
        Some((shared.app.clone(), shared.enabled))
    })
    .flatten();
    let Some((app, enabled)) = changed else {
        return;
    };
    platform::mark_hearing(&app, matches!(status, WakeWordStatus::Hearing { .. }));
    if let Err(error) = (WakeWordChanged { enabled, status }).emit(&app) {
        eprintln!("toodo: failed to emit wake-word-changed: {error}");
    }
}

pub fn set_enabled(app: &AppHandle, enabled: bool) {
    with_shared(|shared| {
        shared.enabled = enabled;
        if !enabled {
            if let Some(worker) = shared.worker.take() {
                worker.control.stop.store(true, Ordering::SeqCst);
            }
            return;
        }
        if shared
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.thread.is_finished())
        {
            return;
        }
        let id = WORKERS.fetch_add(1, Ordering::SeqCst) + 1;
        // Returned by the command that turned it on; the worker reports from here on.
        shared.status = WakeWordStatus::Starting;
        let control = Arc::new(Control::default());
        control.speaking.store(shared.speaking, Ordering::SeqCst);
        control.dictating.store(shared.dictating, Ordering::SeqCst);
        let thread = {
            let (app, dir, control) = (app.clone(), shared.data_dir.clone(), control.clone());
            std::thread::spawn(move || run(&app, id, &dir.join(MODEL_DIRECTORY), &control))
        };
        shared.worker = Some(Worker {
            id,
            control,
            thread,
        });
    });
    if !enabled {
        platform::release(app);
        set_status(None, WakeWordStatus::Off);
    }
}

pub fn cancel() {
    with_shared(|shared| {
        if let Some(worker) = &shared.worker {
            worker.control.cancel.store(true, Ordering::SeqCst);
        }
    });
}

pub fn set_speaking(speaking: bool) {
    with_shared(|shared| {
        shared.speaking = speaking;
        if let Some(worker) = &shared.worker {
            worker.control.speaking.store(speaking, Ordering::SeqCst);
        }
    });
}

pub fn set_dictating(dictating: bool) {
    with_shared(|shared| {
        shared.dictating = dictating;
        if let Some(worker) = &shared.worker {
            worker.control.dictating.store(dictating, Ordering::SeqCst);
        }
    });
}

/// Stops the worker and closes the microphone; runs on the main thread as the app exits.
pub fn shut_down() {
    with_shared(|shared| {
        if let Some(worker) = shared.worker.take() {
            worker.control.stop.store(true, Ordering::SeqCst);
        }
    });
    platform::release_now();
}

fn failed(kind: WakeWordErrorKind, message: impl Into<String>) -> WakeWordStatus {
    WakeWordStatus::Failed {
        error: WakeWordError {
            kind,
            message: message.into(),
        },
    }
}

fn not_allowed() -> WakeWordStatus {
    failed(
        WakeWordErrorKind::MicrophoneNotAllowed,
        platform::NOT_ALLOWED,
    )
}

fn run(app: &AppHandle, id: u32, dir: &Path, control: &Control) {
    let status = |status| set_status(Some(id), status);
    status(WakeWordStatus::Starting);

    if !model::is_ready(dir) {
        let total = download::total_size();
        let shown = AtomicU32::new(u32::MAX);
        let downloaded = tauri::async_runtime::block_on(download::download(dir, |done, total| {
            // One status per percent is plenty.
            let percent = (done * 100 / total.max(1)) as u32;
            if shown.swap(percent, Ordering::Relaxed) != percent {
                status(WakeWordStatus::Downloading {
                    done_bytes: done as f64,
                    total_bytes: total as f64,
                });
            }
            !control.stop.load(Ordering::SeqCst)
        }));
        if let Err(error) = downloaded {
            if !control.stop.load(Ordering::SeqCst) {
                status(failed(
                    WakeWordErrorKind::Download,
                    format!("Cannot download the wake word model ({total} bytes): {error}"),
                ));
            }
            return;
        }
        status(WakeWordStatus::Starting);
    }
    let recognizer = match Sherpa::load(dir) {
        Ok(recognizer) => recognizer,
        Err(message) => return status(failed(WakeWordErrorKind::Failed, message)),
    };
    let mut detector = Detector::new(recognizer);
    let _awake = platform::KeepAwake::begin();

    let (sender, audio) = mpsc::sync_channel::<Audio>(QUEUE);
    let mut microphone = platform::Microphone::default();
    let mut stream = None;
    let mut attached = false;
    let mut last_audio = Instant::now();
    let mut last_try: Option<Instant> = None;
    let mut last_check = Instant::now();
    while !control.stop.load(Ordering::SeqCst) {
        if attached && last_check.elapsed() >= RETRY {
            last_check = Instant::now();
            // A permission taken away while listening: some systems keep delivering
            // silence then rather than stopping the microphone.
            if !platform::microphone_allowed(app) {
                microphone.close();
                attached = false;
                last_try = Some(Instant::now());
                status(not_allowed());
                continue;
            }
        }
        if !attached || last_audio.elapsed() >= SILENT_DEVICE || microphone.broken() {
            if last_try.is_some_and(|at| at.elapsed() < RETRY) {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            last_try = Some(Instant::now());
            if !platform::microphone_allowed(app) {
                status(not_allowed());
                continue;
            }
            match microphone.open(app, sender.clone()) {
                Some(Ok(())) => {
                    attached = true;
                    last_audio = Instant::now();
                    last_check = Instant::now();
                }
                Some(Err(message)) => status(failed(WakeWordErrorKind::NoMicrophone, message)),
                None => break,
            }
            continue;
        }
        let mut listener = Listener {
            app,
            id,
            control,
            detector: &mut detector,
            stream: &mut stream,
        };
        match audio.recv_timeout(Duration::from_millis(200)) {
            Ok(first) => {
                last_audio = Instant::now();
                listener.take(first);
                while let Ok(next) = audio.try_recv() {
                    listener.take(next);
                }
            }
            Err(RecvTimeoutError::Timeout) => listener.idle(),
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    microphone.close();
}

/// The detector and the 16 kHz stream it is fed, kept across buffers.
struct Stream {
    generation: u32,
    rate: f64,
    resampler: Option<LinearResampler>,
    pending: Vec<f32>,
    /// Samples since the microphone opened.
    at: u64,
    holds: Holds,
    listened: bool,
}

struct Listener<'a> {
    app: &'a AppHandle,
    id: u32,
    control: &'a Control,
    detector: &'a mut Detector<Sherpa>,
    stream: &'a mut Option<Stream>,
}

impl Listener<'_> {
    fn status(&self, status: WakeWordStatus) {
        set_status(Some(self.id), status);
    }

    /// Between buffers: a cancel still ends the sentence being heard.
    fn idle(&mut self) {
        if self.control.cancel.swap(false, Ordering::SeqCst) && self.detector.cancel().is_some() {
            self.status(WakeWordStatus::Listening);
        }
    }

    fn take(&mut self, audio: Audio) {
        let same = self
            .stream
            .as_ref()
            .is_some_and(|s| s.generation == audio.generation && s.rate == audio.rate);
        if !same {
            // A new start of the microphone: its clock starts at zero.
            self.detector.restart(0);
            *self.stream = Some(Stream {
                generation: audio.generation,
                rate: audio.rate,
                resampler: (audio.rate as u32 != SAMPLE_RATE)
                    .then(|| LinearResampler::create(audio.rate as i32, SAMPLE_RATE as i32))
                    .flatten(),
                pending: Vec::new(),
                at: 0,
                holds: Holds::default(),
                listened: true,
            });
        }
        let Some(mut stream) = self.stream.take() else {
            return;
        };
        match &stream.resampler {
            Some(resampler) => stream
                .pending
                .extend(resampler.resample(&audio.samples, false)),
            None => stream.pending.extend_from_slice(&audio.samples),
        }
        let whole = stream.pending.len() / CHUNK * CHUNK;
        let ready: Vec<f32> = stream.pending.drain(..whole).collect();
        for chunk in ready.chunks(CHUNK) {
            self.feed(&mut stream, chunk);
        }
        *self.stream = Some(stream);
    }

    fn feed(&mut self, stream: &mut Stream, chunk: &[f32]) {
        let now = stream.at as f32 / SAMPLE_RATE as f32;
        stream.at += chunk.len() as u64;
        stream
            .holds
            .set_speaking(self.control.speaking.load(Ordering::SeqCst), now);
        stream
            .holds
            .set_dictating(platform::dictating() || self.control.dictating.load(Ordering::SeqCst));
        let cancel = self.control.cancel.swap(false, Ordering::SeqCst);
        let listens = stream.holds.listens(now);
        if cancel || !listens {
            self.detector.cancel();
        }
        if !listens {
            stream.listened = false;
            return self.status(WakeWordStatus::Held);
        }
        if !stream.listened {
            // Back from a hold: what was said meanwhile does not count.
            stream.listened = true;
            self.detector.restart(stream.at - chunk.len() as u64);
        }
        for event in self.detector.feed(chunk) {
            match event {
                Heard::Wake(_) => {
                    platform::play_cue(self.app);
                    self.status(WakeWordStatus::Hearing {
                        transcript: String::new(),
                    });
                }
                Heard::Heard(transcript) => self.status(WakeWordStatus::Hearing { transcript }),
                Heard::Done(outcome) => {
                    if outcome.sends() {
                        let heard = WakeWordHeard {
                            id: HEARD.fetch_add(1, Ordering::SeqCst) + 1,
                            message: outcome.message,
                        };
                        if let Err(error) = heard.emit(self.app) {
                            eprintln!("toodo: failed to emit wake-word-heard: {error}");
                        }
                    }
                }
            }
        }
        if !self.detector.is_hearing() {
            self.status(WakeWordStatus::Listening);
        }
    }
}

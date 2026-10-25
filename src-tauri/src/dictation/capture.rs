//! The one owner of the microphone in the macOS app: a single `AVAudioEngine` whose input
//! tap hands every buffer to whoever listens, a dictated message and the wake word. It runs
//! while either listens and stops, closing the microphone, when neither does. Everything
//! here runs on the main thread; the tap block runs on the audio thread.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use objc2_speech::SFSpeechAudioBufferRecognitionRequest;

/// Mono audio from the microphone for the wake word, at the device's rate.
pub struct Audio {
    /// Which start of the engine it came from; counting restarts at zero with each.
    pub generation: u32,
    pub rate: f64,
    pub samples: Vec<f32>,
}

/// Whether a dictated message has the microphone; the wake word holds off meanwhile.
pub static DICTATING: AtomicBool = AtomicBool::new(false);

static GENERATION: AtomicU32 = AtomicU32::new(0);

/// A dictation's recognition request, which the audio thread appends buffers to.
struct Request(Retained<SFSpeechAudioBufferRecognitionRequest>);

// SAFETY: a buffer recognition request takes appended buffers from any thread (dictation
// has always fed it from the tap's audio thread); here it is only appended to, under the
// sinks' mutex, and released on the main thread.
unsafe impl Send for Request {}
// SAFETY: as above; shared access only appends under the mutex.
unsafe impl Sync for Request {}

#[derive(Default)]
struct Sinks {
    dictation: Option<Request>,
    wake: Option<SyncSender<Audio>>,
}

struct Capture {
    engine: Retained<AVAudioEngine>,
    sinks: Arc<Mutex<Sinks>>,
}

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

/// Why the microphone could not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    NoMicrophone,
    DidNotStart,
}

fn sinks() -> Option<Arc<Mutex<Sinks>>> {
    CAPTURE.with_borrow(|capture| capture.as_ref().map(|capture| capture.sinks.clone()))
}

fn lock(sinks: &Mutex<Sinks>) -> std::sync::MutexGuard<'_, Sinks> {
    sinks.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a fresh engine on the default input device with the given listeners.
fn open(sinks: Sinks) -> Result<(), (StartError, String)> {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let sinks = Arc::new(Mutex::new(sinks));
    // SAFETY: on the main thread with valid arguments; the tap block only reads the buffer
    // it is given and appends it to a request, which accepts buffers from any thread.
    unsafe {
        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        let format = input.outputFormatForBus(0);
        if format.channelCount() == 0 || format.sampleRate() <= 0.0 {
            return Err((
                StartError::NoMicrophone,
                "No microphone was found.".to_owned(),
            ));
        }
        let rate = format.sampleRate();
        let tap_sinks = sinks.clone();
        let tap = RcBlock::new(
            move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                let buffer = buffer.as_ref();
                let sinks = lock(&tap_sinks);
                if let Some(Request(request)) = &sinks.dictation {
                    request.appendAudioPCMBuffer(buffer);
                }
                if let Some(wake) = &sinks.wake {
                    let channels = buffer.floatChannelData();
                    if !channels.is_null() {
                        let first = (*channels).as_ptr();
                        let frames = buffer.frameLength() as usize;
                        let samples = std::slice::from_raw_parts(first, frames).to_vec();
                        // A full queue means the detector fell behind; dropping a buffer
                        // beats blocking the audio thread.
                        let _ = wake.try_send(Audio {
                            generation,
                            rate,
                            samples,
                        });
                    }
                }
            },
        );
        input.installTapOnBus_bufferSize_format_block(
            0,
            1024,
            Some(&format),
            RcBlock::as_ptr(&tap),
        );
        engine.prepare();
        if let Err(error) = engine.startAndReturnError() {
            input.removeTapOnBus(0);
            return Err((
                StartError::DidNotStart,
                format!(
                    "The microphone did not start: {}",
                    error.localizedDescription()
                ),
            ));
        }
        CAPTURE.set(Some(Capture { engine, sinks }));
    }
    Ok(())
}

fn close() {
    if let Some(capture) = CAPTURE.take() {
        // SAFETY: on the main thread, as `open` made it; removing the tap it installed.
        unsafe {
            capture.engine.stop();
            capture.engine.inputNode().removeTapOnBus(0);
        }
    }
}

/// Applies `change` to the listeners, starting the engine for the first one and stopping
/// it after the last one leaves.
fn update(change: impl FnOnce(&mut Sinks)) -> Result<(), (StartError, String)> {
    match sinks() {
        Some(sinks) => {
            let empty = {
                let mut sinks = lock(&sinks);
                change(&mut sinks);
                sinks.dictation.is_none() && sinks.wake.is_none()
            };
            if empty {
                close();
            }
            Ok(())
        }
        None => {
            let mut sinks = Sinks::default();
            change(&mut sinks);
            if sinks.dictation.is_none() && sinks.wake.is_none() {
                Ok(())
            } else {
                open(sinks)
            }
        }
    }
}

/// Sends the microphone to a dictated message's recognition request.
pub fn attach_dictation(
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
) -> Result<(), (StartError, String)> {
    DICTATING.store(true, Ordering::SeqCst);
    let result = update(|sinks| sinks.dictation = Some(Request(request)));
    if result.is_err() {
        DICTATING.store(false, Ordering::SeqCst);
    }
    result
}

pub fn detach_dictation() {
    DICTATING.store(false, Ordering::SeqCst);
    let _ = update(|sinks| sinks.dictation = None);
}

/// Sends the microphone to the wake word detector.
pub fn attach_wake(sender: SyncSender<Audio>) -> Result<(), (StartError, String)> {
    update(|sinks| sinks.wake = Some(sender))
}

pub fn detach_wake() {
    let _ = update(|sinks| sinks.wake = None);
}

/// Opens the default input device again with the same listeners: after the device changed
/// or the Mac woke from sleep, when the old engine stopped delivering audio.
pub fn reopen() -> Result<(), (StartError, String)> {
    let Some(sinks) = sinks() else {
        return Ok(());
    };
    let listeners = std::mem::take(&mut *lock(&sinks));
    close();
    open(listeners)
}

//! Dictation with `AVAudioEngine` and `SFSpeechRecognizer`. The engine and the recognition
//! task live on the main thread; the system calls the permission, audio tap, and result
//! blocks on queues of its own, which only report through events.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::{NSError, NSLocale, NSString};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};
use tauri::AppHandle;
use tauri_specta::Event;

use super::{DictationChanged, DictationError, DictationErrorKind};

/// The session whose events still count; 0 when none. Whoever swaps a session out of here
/// sends its last event, so every session ends exactly once.
static CURRENT: AtomicU32 = AtomicU32::new(0);

/// `kAFAssistantErrorDomain` 1110: the recognizer heard no speech.
const NO_SPEECH_DOMAIN: &str = "kAFAssistantErrorDomain";
const NO_SPEECH_CODE: isize = 1110;

struct Session {
    id: u32,
    engine: Retained<AVAudioEngine>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
    _recognizer: Retained<SFSpeechRecognizer>,
}

thread_local! {
    /// The running session, kept on the main thread.
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

pub fn start(app: &AppHandle, id: u32, language: String) -> tauri::Result<()> {
    let previous = CURRENT.swap(id, Ordering::SeqCst);
    if previous != 0 && previous != id {
        send(app, previous, String::new(), true, None);
    }
    let handle = app.clone();
    app.run_on_main_thread(move || {
        release(None);
        begin(&handle, id, language);
    })
}

pub fn stop(app: &AppHandle, id: u32) -> tauri::Result<()> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let listening = SESSION.with_borrow(|session| match session {
            Some(session) if session.id == id => {
                // SAFETY: the engine and request are used on the main thread they were
                // made on; the tap is the one `listen` installed on bus 0.
                unsafe {
                    session.engine.stop();
                    session.engine.inputNode().removeTapOnBus(0);
                    session.request.endAudio();
                }
                true
            }
            _ => false,
        });
        // Still waiting for a permission answer: nothing was heard yet.
        if !listening {
            finish(&handle, id, String::new(), None);
        }
    })
}

pub fn cancel(app: &AppHandle, id: u32) -> tauri::Result<()> {
    finish(app, id, String::new(), None);
    app.run_on_main_thread(move || release(Some(id)))
}

fn send(
    app: &AppHandle,
    session: u32,
    transcript: String,
    done: bool,
    error: Option<DictationError>,
) {
    let event = DictationChanged {
        session,
        transcript,
        done,
        error,
    };
    if let Err(error) = event.emit(app) {
        eprintln!("toodo: failed to emit dictation-changed: {error}");
    }
}

/// Ends session `id` if it is still the current one, then frees its audio.
fn finish(app: &AppHandle, id: u32, transcript: String, error: Option<DictationError>) {
    if CURRENT
        .compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        send(app, id, transcript, true, error);
        if let Err(error) = app.run_on_main_thread(move || release(Some(id))) {
            eprintln!("toodo: cannot free the dictation audio: {error}");
        }
    }
}

fn fail(app: &AppHandle, id: u32, kind: DictationErrorKind, message: impl Into<String>) {
    let error = DictationError {
        kind,
        message: message.into(),
    };
    finish(app, id, String::new(), Some(error));
}

/// Stops and drops the running session: any session for `None`, else only session `id`.
fn release(id: Option<u32>) {
    let session = SESSION.with_borrow_mut(|session| {
        if id.is_none_or(|id| session.as_ref().is_some_and(|session| session.id == id)) {
            session.take()
        } else {
            None
        }
    });
    if let Some(session) = session {
        // SAFETY: on the main thread, as `listen` made them; removing a tap that is gone
        // already and cancelling a finished task do nothing.
        unsafe {
            session.engine.stop();
            session.engine.inputNode().removeTapOnBus(0);
            session.task.cancel();
        }
    }
}

/// Asks for what is missing, one permission at a time, then listens. Runs on the main
/// thread; a permission answer comes back here through `run_on_main_thread`.
fn begin(app: &AppHandle, id: u32, language: String) {
    if CURRENT.load(Ordering::SeqCst) != id {
        return;
    }
    // SAFETY: plain class methods without arguments.
    match unsafe { SFSpeechRecognizer::authorizationStatus() } {
        SFSpeechRecognizerAuthorizationStatus::Authorized => {}
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => {
            let app = app.clone();
            let answer = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
                if status == SFSpeechRecognizerAuthorizationStatus::Authorized {
                    resume(&app, id, language.clone());
                } else {
                    speech_not_allowed(&app, id);
                }
            });
            // SAFETY: the block takes the status argument the method passes.
            unsafe { SFSpeechRecognizer::requestAuthorization(&answer) };
            return;
        }
        _ => return speech_not_allowed(app, id),
    }

    // SAFETY: a static of AVFoundation, set once the framework is loaded.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return fail(
            app,
            id,
            DictationErrorKind::Failed,
            "AVMediaTypeAudio is missing",
        );
    };
    // SAFETY: `audio` is a media type AVCaptureDevice accepts.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => {}
        AVAuthorizationStatus::NotDetermined => {
            let app = app.clone();
            let answer = RcBlock::new(move |granted: Bool| {
                if granted.as_bool() {
                    resume(&app, id, language.clone());
                } else {
                    microphone_not_allowed(&app, id);
                }
            });
            // SAFETY: as above, with the block taking the granted flag.
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &answer) };
            return;
        }
        _ => return microphone_not_allowed(app, id),
    }

    match listen(app, id, &language) {
        Ok(session) => {
            SESSION.set(Some(session));
            // Cancelled while the audio started.
            if CURRENT.load(Ordering::SeqCst) != id {
                release(Some(id));
            }
        }
        Err(error) => finish(app, id, String::new(), Some(error)),
    }
}

fn resume(app: &AppHandle, id: u32, language: String) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || begin(&handle, id, language)) {
        fail(app, id, DictationErrorKind::Failed, error.to_string());
    }
}

fn speech_not_allowed(app: &AppHandle, id: u32) {
    fail(
        app,
        id,
        DictationErrorKind::SpeechNotAllowed,
        "Speech recognition is turned off for Toodo. Allow it in System Settings > Privacy & Security > Speech Recognition.",
    );
}

fn microphone_not_allowed(app: &AppHandle, id: u32) {
    fail(
        app,
        id,
        DictationErrorKind::MicrophoneNotAllowed,
        "The microphone is turned off for Toodo. Allow it in System Settings > Privacy & Security > Microphone.",
    );
}

fn failed(message: impl Into<String>) -> DictationError {
    DictationError {
        kind: DictationErrorKind::Failed,
        message: message.into(),
    }
}

/// Starts the microphone and a recognition task that reports to the frontend.
fn listen(app: &AppHandle, id: u32, language: &str) -> Result<Session, DictationError> {
    let locale =
        NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(language));
    // SAFETY: a fresh allocation and a valid locale.
    let recognizer =
        unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale) }
            .ok_or_else(|| DictationError {
                kind: DictationErrorKind::LanguageNotSupported,
                message: format!("Speech recognition does not support {language} on this Mac."),
            })?;
    // SAFETY: the methods below are called on the main thread with valid arguments; the
    // tap block appends buffers, which a buffer request accepts from any thread.
    unsafe {
        if !recognizer.isAvailable() {
            return Err(failed(
                "Speech recognition is not available right now. Check the network connection, and that Dictation is on in System Settings > Keyboard.",
            ));
        }
        let request = SFSpeechAudioBufferRecognitionRequest::new();
        request.setShouldReportPartialResults(true);

        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        let format = input.outputFormatForBus(0);
        if format.channelCount() == 0 || format.sampleRate() <= 0.0 {
            return Err(DictationError {
                kind: DictationErrorKind::NoMicrophone,
                message: "No microphone was found.".to_owned(),
            });
        }
        let tap_request = request.clone();
        let tap = RcBlock::new(
            move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                tap_request.appendAudioPCMBuffer(buffer.as_ref());
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
            return Err(failed(format!(
                "The microphone did not start: {}",
                error.localizedDescription()
            )));
        }

        let heard = Arc::new(Mutex::new(String::new()));
        let app = app.clone();
        let report = RcBlock::new(
            move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                if let Some(result) = result.as_ref() {
                    let transcript = result.bestTranscription().formattedString().to_string();
                    heard
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone_from(&transcript);
                    if result.isFinal() {
                        finish(&app, id, transcript, None);
                    } else if CURRENT.load(Ordering::SeqCst) == id {
                        send(&app, id, transcript, false, None);
                    }
                } else if let Some(error) = error.as_ref() {
                    let transcript = heard.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    // Words heard before the error still make a message.
                    if !transcript.is_empty() {
                        finish(&app, id, transcript, None);
                    } else if no_speech(error) {
                        let error = DictationError {
                            kind: DictationErrorKind::NoSpeech,
                            message: "No speech was heard. Try again.".to_owned(),
                        };
                        finish(&app, id, String::new(), Some(error));
                    } else {
                        let message = format!(
                            "Speech recognition failed: {}",
                            error.localizedDescription()
                        );
                        finish(&app, id, String::new(), Some(failed(message)));
                    }
                }
            },
        );
        let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &report);
        Ok(Session {
            id,
            engine,
            request,
            task,
            _recognizer: recognizer,
        })
    }
}

fn no_speech(error: &NSError) -> bool {
    error.code() == NO_SPEECH_CODE && error.domain().to_string() == NO_SPEECH_DOMAIN
}

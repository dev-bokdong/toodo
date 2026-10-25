//! The macOS side of the wake word worker ([`super::worker`]): the microphone from the app's
//! one owner ([`capture`]), shared with dictation, the microphone permission, the cue, the
//! menu bar title, and keeping macOS from napping the app while it listens.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, SyncSender};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObjectProtocol, ProtocolObject};
use objc2_app_kit::NSSound;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
use tauri::AppHandle;

use crate::dictation::capture;
pub use crate::dictation::capture::Audio;

pub const NOT_ALLOWED: &str = "The microphone is turned off for Toodo. Allow it in System Settings > Privacy & Security > Microphone.";

/// The system sound played when the wake word is heard.
const CUE: &str = "Tink";
const TRAY_ID: &str = "main";

/// Runs `f` on the main thread and waits for its result.
fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .ok()?;
    rx.recv().ok()
}

/// Whether the app may use the microphone, asking the user if they have not answered yet.
pub fn microphone_allowed(app: &AppHandle) -> bool {
    // SAFETY: a static of AVFoundation, set once the framework is loaded.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return false;
    };
    // SAFETY: `audio` is a media type AVCaptureDevice accepts.
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => true,
        AVAuthorizationStatus::NotDetermined => {
            let (tx, rx) = mpsc::channel();
            let asked = on_main(app, move || {
                let answer = RcBlock::new(move |granted: Bool| {
                    let _ = tx.send(granted.as_bool());
                });
                // SAFETY: as above; the block takes the granted flag the method passes.
                if let Some(audio) = unsafe { AVMediaTypeAudio } {
                    unsafe {
                        AVCaptureDevice::requestAccessForMediaType_completionHandler(
                            audio, &answer,
                        );
                    }
                }
            });
            asked.is_some() && rx.recv().unwrap_or(false)
        }
        _ => false,
    }
}

/// The wake word's place on the app's one microphone engine.
#[derive(Default)]
pub struct Microphone {
    attached: bool,
}

impl Microphone {
    /// Starts sending the microphone to `sender`, or opens the default input device again
    /// with the same listeners once attached. `None` when the app is gone.
    pub fn open(
        &mut self,
        app: &AppHandle,
        sender: SyncSender<Audio>,
    ) -> Option<Result<(), String>> {
        let attached = self.attached;
        let opened = on_main(app, move || {
            if attached {
                capture::reopen()
            } else {
                capture::attach_wake(sender)
            }
        })?;
        Some(match opened {
            Ok(()) => {
                self.attached = true;
                Ok(())
            }
            Err((_, message)) => Err(message),
        })
    }

    /// The engine reports nothing; a stopped one shows by its silence.
    pub fn broken(&self) -> bool {
        false
    }

    /// The engine is shared with dictation and stays; [`release`] detaches the wake word.
    pub fn close(&mut self) {}
}

/// Takes the wake word off the microphone, from any thread.
pub fn release(app: &AppHandle) {
    if let Err(error) = app.run_on_main_thread(capture::detach_wake) {
        eprintln!("toodo: cannot close the microphone: {error}");
    }
}

/// Takes the wake word off the microphone; on the main thread, as the app exits.
pub fn release_now() {
    capture::detach_wake();
}

/// Whether a dictated message has the microphone.
pub fn dictating() -> bool {
    capture::DICTATING.load(Ordering::SeqCst)
}

pub fn play_cue(app: &AppHandle) {
    let _ = app.run_on_main_thread(|| {
        if let Some(sound) = NSSound::soundNamed(&NSString::from_str(CUE)) {
            sound.play();
        }
    });
}

/// Shows "Listening" beside the menu bar icon while a sentence is heard.
pub fn mark_hearing(app: &AppHandle, hearing: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID)
        && let Err(error) = tray.set_title(hearing.then_some("Listening"))
    {
        eprintln!("toodo: cannot mark the menu bar icon: {error}");
    }
}

/// Keeps macOS from napping the app while it listens with its window hidden, for as long as
/// it lives.
pub struct KeepAwake {
    process: Retained<NSProcessInfo>,
    activity: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl KeepAwake {
    pub fn begin() -> Self {
        let reason = NSString::from_str("Listening for the wake word");
        let process = NSProcessInfo::processInfo();
        let activity = process.beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
            &reason,
        );
        Self { process, activity }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        // SAFETY: the activity `begin` began.
        unsafe { self.process.endActivity(&self.activity) };
    }
}

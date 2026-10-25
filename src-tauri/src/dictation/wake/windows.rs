//! The Windows side of the wake word worker ([`super::worker`]): the default input device
//! ([`super::microphone`]), the microphone switches of Windows' privacy settings, the cue, the
//! tray icon's tooltip, and keeping Windows from slowing the listening thread down while the
//! window is hidden.

use std::sync::mpsc::SyncSender;

use tauri::AppHandle;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, SetThreadInformation, THREAD_POWER_THROTTLING_CURRENT_VERSION,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE, ThreadPowerThrottling,
};
use windows_sys::Win32::UI::WindowsAndMessaging::MB_OK;

pub use super::microphone::Audio;

/// The wake word's stream from the default input device, closed when dropped.
#[derive(Default)]
pub struct Microphone(super::microphone::Microphone);

impl Microphone {
    /// Opens the default input device, anew if it was open, and sends its audio to `sender`.
    pub fn open(
        &mut self,
        _app: &AppHandle,
        sender: SyncSender<Audio>,
    ) -> Option<Result<(), String>> {
        Some(self.0.open(sender))
    }

    pub fn broken(&self) -> bool {
        self.0.broken()
    }

    pub fn close(&mut self) {
        self.0.close();
    }
}

pub const NOT_ALLOWED: &str = "The microphone is turned off for desktop apps. Allow it in Settings > Privacy & security > Microphone.";

const TRAY_ID: &str = "main";
/// Where Windows keeps the microphone switches of its privacy settings: in the machine's
/// key "Microphone access", in the user's key "Let apps access your microphone", and in its
/// `NonPackaged` subkey "Let desktop apps access your microphone".
const CONSENT: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The `Value` of a consent key, `Allow` or `Deny`; `None` when it was never set.
fn consent(root: HKEY, key: &str) -> Option<String> {
    let key = wide(key);
    let name = wide("Value");
    let mut value = [0u16; 16];
    let mut size = std::mem::size_of_val(&value) as u32;
    // SAFETY: the key and value names are NUL-terminated, and `size` is the byte size of
    // the buffer the value is written to.
    let status = unsafe {
        RegGetValueW(
            root,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let units = (size as usize / 2).min(value.len());
    Some(
        String::from_utf16_lossy(&value[..units])
            .trim_end_matches('\0')
            .to_owned(),
    )
}

/// Whether Windows lets desktop apps use the microphone. Windows asks nothing of a desktop
/// app; with a switch off, its microphone delivers only silence.
pub fn microphone_allowed(_app: &AppHandle) -> bool {
    let desktop = format!(r"{CONSENT}\NonPackaged");
    super::consent_allows(&[
        consent(HKEY_LOCAL_MACHINE, CONSENT),
        consent(HKEY_CURRENT_USER, CONSENT),
        consent(HKEY_CURRENT_USER, &desktop),
    ])
}

/// Nothing to do: the worker's stream closes as the worker stops.
pub fn release(_app: &AppHandle) {}

/// Nothing to do: the worker's stream closes with the app.
pub fn release_now() {}

/// Dictation in the lead chat runs in the WebView, which tells the worker itself.
pub fn dictating() -> bool {
    false
}

pub fn play_cue(_app: &AppHandle) {
    // SAFETY: plays the default sound asynchronously; any style value is valid.
    unsafe { MessageBeep(MB_OK) };
}

/// Shows that a sentence is heard in the tray icon's tooltip.
pub fn mark_hearing(app: &AppHandle, hearing: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tooltip = if hearing {
            "Toodo - Listening"
        } else {
            "Toodo"
        };
        if let Err(error) = tray.set_tooltip(Some(tooltip)) {
            eprintln!("toodo: cannot mark the tray icon: {error}");
        }
    }
}

/// Opts the listening thread out of Windows' power throttling, which slows down the threads
/// of an app without a visible window and would let the detector fall behind the microphone.
pub struct KeepAwake;

impl KeepAwake {
    pub fn begin() -> Self {
        let state = THREAD_POWER_THROTTLING_STATE {
            Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0,
        };
        // SAFETY: `state` is a valid THREAD_POWER_THROTTLING_STATE of the given size, for the
        // calling thread's pseudo handle.
        let set = unsafe {
            SetThreadInformation(
                GetCurrentThread(),
                ThreadPowerThrottling,
                (&raw const state).cast(),
                std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
            )
        };
        if set == 0 {
            eprintln!(
                "toodo: cannot opt the wake word out of power throttling: {}",
                std::io::Error::last_os_error()
            );
        }
        Self
    }
}

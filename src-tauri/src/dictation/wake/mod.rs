//! The wake word of the macOS and Windows apps: saying "do-ri-ya" and a sentence sends the
//! sentence to the lead chat hands-free, while the app runs with its window shown or hidden
//! in the menu bar or the tray. It is off until turned on in the settings, listens only while
//! the app runs (no login item, service, or system-wide hotword), and closes the microphone on
//! Quit or when turned off.
//!
//! The detector is the measured one of docs/wake-word.md ([`rule`], [`detector`],
//! [`engine`]), run by one [`worker`] for both apps on the platform's microphone: the macOS
//! app's one microphone owner ([`super::capture`]), the Windows app's default input device.
//! It reports what it hears as [`WakeWordChanged`] events and each sentence to send as a
//! [`WakeWordHeard`] event; the frontend sends it to the lead chat, so it goes to the server
//! the app uses.

#[cfg(any(target_os = "macos", windows, test))]
mod detector;
#[cfg(any(target_os = "macos", windows))]
mod download;
#[cfg(any(target_os = "macos", windows))]
mod engine;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod microphone;
#[cfg(any(target_os = "macos", windows, test))]
mod model;
#[cfg(any(target_os = "macos", windows, test))]
mod rule;
#[cfg(windows)]
mod windows;
#[cfg(any(target_os = "macos", windows))]
mod worker;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::commands::{CommandError, CommandResult};

/// Where the wake word stands, sent whenever it changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct WakeWordChanged {
    /// The setting: whether the app listens for the wake word while it runs.
    pub enabled: bool,
    pub status: WakeWordStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum WakeWordStatus {
    Off,
    /// The recognizer is being downloaded, the first time the wake word is turned on.
    #[serde(rename_all = "camelCase")]
    Downloading {
        done_bytes: f64,
        total_bytes: f64,
    },
    /// The recognizer loads and the microphone opens.
    Starting,
    Listening,
    /// Not listening while the app reads a reply aloud or a message is dictated.
    Held,
    /// The wake word was heard; the sentence after it so far.
    Hearing {
        transcript: String,
    },
    Failed {
        error: WakeWordError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WakeWordError {
    pub kind: WakeWordErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum WakeWordErrorKind {
    /// The microphone is turned off for the app (on Windows, for desktop apps) in the
    /// system's privacy settings; listening starts again by itself once it is allowed.
    MicrophoneNotAllowed,
    /// No microphone, or it stopped; the app keeps trying to open the default one.
    NoMicrophone,
    Download,
    Failed,
}

/// A sentence said after the wake word, for the lead chat. Each is sent once: the frontend
/// gives it one client ID, kept across retries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct WakeWordHeard {
    /// Counts the sentences heard since the app started.
    pub id: u32,
    pub message: String,
}

/// The file in the app data directory that keeps the setting.
#[cfg(any(target_os = "macos", windows, test))]
const SETTING_FILE: &str = "wake-word.json";

#[cfg(any(target_os = "macos", windows, test))]
#[derive(Debug, Default, Serialize, Deserialize)]
struct Setting {
    enabled: bool,
}

/// Whether the wake word is turned on; off unless it was turned on and saved.
#[cfg(any(target_os = "macos", windows, test))]
fn load_enabled(data_dir: &std::path::Path) -> bool {
    std::fs::read(data_dir.join(SETTING_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Setting>(&bytes).ok())
        .is_some_and(|setting| setting.enabled)
}

#[cfg(any(target_os = "macos", windows, test))]
fn save_enabled(data_dir: &std::path::Path, enabled: bool) -> std::io::Result<()> {
    let json = serde_json::to_vec(&Setting { enabled }).expect("the setting serializes");
    std::fs::write(data_dir.join(SETTING_FILE), json)
}

/// Whether Windows' privacy settings let a desktop app use the microphone, from the `Value`
/// of each of its microphone switches; a switch never set allows.
#[cfg(any(windows, test))]
fn consent_allows(values: &[Option<String>]) -> bool {
    !values
        .iter()
        .flatten()
        .any(|value| value.trim().eq_ignore_ascii_case("deny"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn unsupported() -> CommandError {
    CommandError::rejected("the wake word is only available in the macOS and Windows apps")
}

/// Starts listening at app start if the wake word was turned on, for an app connected to a
/// server (whose lead takes the sentences).
#[cfg(any(target_os = "macos", windows))]
pub fn start_if_enabled(app: &tauri::AppHandle, data_dir: &std::path::Path, connected: bool) {
    worker::init(app, data_dir.to_owned());
    if connected && load_enabled(data_dir) {
        worker::set_enabled(app, true);
    }
}

/// Closes the microphone as the app exits.
#[cfg(any(target_os = "macos", windows))]
pub fn shut_down() {
    worker::shut_down();
}

#[tauri::command]
#[specta::specta]
pub fn get_wake_word() -> CommandResult<WakeWordChanged> {
    #[cfg(any(target_os = "macos", windows))]
    {
        Ok(worker::current())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(unsupported())
    }
}

/// Turns the wake word on or off and keeps the setting. Turning it on downloads the
/// recognizer the first time and asks for the microphone if the user has not answered yet.
#[tauri::command]
#[specta::specta]
pub fn set_wake_word(app: tauri::AppHandle, enabled: bool) -> CommandResult<WakeWordChanged> {
    #[cfg(any(target_os = "macos", windows))]
    {
        let data_dir = worker::data_dir()
            .ok_or_else(|| CommandError::internal("the wake word is not set up"))?;
        save_enabled(&data_dir, enabled)
            .map_err(|error| CommandError::internal(format!("cannot save the setting: {error}")))?;
        worker::set_enabled(&app, enabled);
        Ok(worker::current())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (app, enabled);
        Err(unsupported())
    }
}

/// Drops the sentence being heard after the wake word, and listens again.
#[tauri::command]
#[specta::specta]
pub fn cancel_wake_word() -> CommandResult<()> {
    #[cfg(any(target_os = "macos", windows))]
    {
        worker::cancel();
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(unsupported())
    }
}

/// Tells the detector whether the app is reading a reply aloud, so its own voice cannot
/// wake it: detection holds while it speaks and for a moment after.
#[tauri::command]
#[specta::specta]
pub fn set_wake_word_speaking(speaking: bool) -> CommandResult<()> {
    #[cfg(any(target_os = "macos", windows))]
    {
        worker::set_speaking(speaking);
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = speaking;
        Err(unsupported())
    }
}

/// Tells the detector whether a message is dictated in the lead chat by the WebView's speech
/// recognition (the Windows app's), so the dictated words cannot also wake it: detection
/// holds while it listens.
#[tauri::command]
#[specta::specta]
pub fn set_wake_word_dictating(dictating: bool) -> CommandResult<()> {
    #[cfg(any(target_os = "macos", windows))]
    {
        worker::set_dictating(dictating);
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = dictating;
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wake_word_is_off_until_turned_on() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!load_enabled(dir.path()));
        std::fs::write(dir.path().join(SETTING_FILE), "not json").unwrap();
        assert!(!load_enabled(dir.path()));
        save_enabled(dir.path(), true).unwrap();
        assert!(load_enabled(dir.path()));
        save_enabled(dir.path(), false).unwrap();
        assert!(!load_enabled(dir.path()));
    }

    #[test]
    fn any_microphone_switch_turned_off_denies() {
        let set = |value: &str| Some(value.to_owned());
        assert!(consent_allows(&[None, None, None]));
        assert!(consent_allows(&[set("Allow"), set("Allow"), set("Allow")]));
        assert!(!consent_allows(&[set("Allow"), None, set("Deny")]));
        assert!(!consent_allows(&[set("deny "), set("Allow"), None]));
        assert!(consent_allows(&[set("Prompt"), None, set("Allow")]));
    }

    #[test]
    fn statuses_are_tagged_for_the_frontend() {
        let changed = WakeWordChanged {
            enabled: true,
            status: WakeWordStatus::Downloading {
                done_bytes: 1.0,
                total_bytes: 2.0,
            },
        };
        assert_eq!(
            serde_json::to_value(&changed).unwrap(),
            serde_json::json!({
                "enabled": true,
                "status": { "state": "downloading", "doneBytes": 1.0, "totalBytes": 2.0 },
            })
        );
        let failed = WakeWordStatus::Failed {
            error: WakeWordError {
                kind: WakeWordErrorKind::MicrophoneNotAllowed,
                message: "off".to_owned(),
            },
        };
        assert_eq!(
            serde_json::to_value(&failed).unwrap(),
            serde_json::json!({
                "state": "failed",
                "error": { "kind": "microphoneNotAllowed", "message": "off" },
            })
        );
    }
}

//! Native dictation for the macOS app. The macOS WebView (WKWebView) gives a page no
//! working speech recognition, so the app listens itself: `AVAudioEngine` takes the
//! microphone and `SFSpeechRecognizer` turns it into text in the chosen language. The
//! frontend starts and stops a session and gets its text as [`DictationChanged`] events.
//!
//! The audio runs in the app, not in the page, so it keeps going while the window is
//! hidden in the menu bar. Other platforms have no native dictation: the commands reject
//! and the frontend uses the browser's speech recognition there (see `src/lib/voice.ts`).
//! The microphone has one owner ([`capture`]), shared with the [`wake`] word.

#[cfg(target_os = "macos")]
mod capture;
#[cfg(target_os = "macos")]
mod macos;
pub mod wake;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::commands::{CommandError, CommandResult};

/// What a dictation session heard so far, or that it ended. Every session ends with one
/// event where `done` is true; a superseded or cancelled session ends without text.
#[derive(Debug, Clone, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct DictationChanged {
    /// The id the frontend gave the session when starting it.
    pub session: u32,
    /// Everything heard in the session, not just the newest words.
    pub transcript: String,
    pub done: bool,
    pub error: Option<DictationError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DictationError {
    pub kind: DictationErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum DictationErrorKind {
    /// Speech recognition is turned off for the app in the system's privacy settings.
    SpeechNotAllowed,
    /// The microphone is turned off for the app in the system's privacy settings.
    MicrophoneNotAllowed,
    NoMicrophone,
    LanguageNotSupported,
    NoSpeech,
    Failed,
}

/// A pane of the system's privacy settings, where a denied permission is given back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PrivacyPane {
    Microphone,
    SpeechRecognition,
}

/// Whether this build of the app has native dictation.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

#[cfg(not(target_os = "macos"))]
fn unsupported() -> CommandError {
    CommandError::rejected("native dictation is only available in the macOS app")
}

/// Checks a BCP 47 language tag such as `ko-KR` before it reaches the system.
fn language_tag(language: &str) -> CommandResult<&str> {
    let tag = language.trim();
    let valid = !tag.is_empty()
        && tag.len() <= 35
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(tag)
    } else {
        Err(CommandError::invalid_input(format!(
            "not a language tag: {language:?}"
        )))
    }
}

/// Starts listening in `language` as session `session`, asking for the speech recognition
/// and microphone permissions first if the user has not answered yet. A session still
/// running ends first.
#[tauri::command]
#[specta::specta]
pub fn start_dictation(app: tauri::AppHandle, session: u32, language: String) -> CommandResult<()> {
    let language = language_tag(&language)?.to_owned();
    if session == 0 {
        return Err(CommandError::invalid_input("session ids start at 1"));
    }
    #[cfg(target_os = "macos")]
    {
        macos::start(&app, session, language)
            .map_err(|error| CommandError::internal(error.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, language);
        Err(unsupported())
    }
}

/// Stops listening; the session ends once the last words are recognized.
#[tauri::command]
#[specta::specta]
pub fn stop_dictation(app: tauri::AppHandle, session: u32) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        macos::stop(&app, session).map_err(|error| CommandError::internal(error.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, session);
        Err(unsupported())
    }
}

/// Stops listening and ends the session at once, without its text.
#[tauri::command]
#[specta::specta]
pub fn cancel_dictation(app: tauri::AppHandle, session: u32) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        macos::cancel(&app, session).map_err(|error| CommandError::internal(error.to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, session);
        Err(unsupported())
    }
}

/// Opens the system's privacy settings at `pane`, where the user can allow what dictation
/// was denied.
#[tauri::command]
#[specta::specta]
pub fn open_privacy_settings(pane: PrivacyPane) -> CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        let anchor = match pane {
            PrivacyPane::Microphone => "Privacy_Microphone",
            PrivacyPane::SpeechRecognition => "Privacy_SpeechRecognition",
        };
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
        std::process::Command::new("open")
            .arg(&url)
            .status()
            .map_err(|error| CommandError::external(format!("cannot open {url}: {error}")))
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(CommandError::external(format!(
                        "open {url} failed: {status}"
                    )))
                }
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags_are_checked() {
        assert_eq!(language_tag(" ko-KR ").unwrap(), "ko-KR");
        assert_eq!(language_tag("en_US").unwrap(), "en_US");
        for bad in ["", "  ", "ko KR", "ko-KR;rm", &"a".repeat(36)] {
            assert!(language_tag(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn events_use_camel_case_fields() {
        let event = DictationChanged {
            session: 3,
            transcript: "hello".to_owned(),
            done: true,
            error: Some(DictationError {
                kind: DictationErrorKind::SpeechNotAllowed,
                message: "denied".to_owned(),
            }),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "session": 3,
                "transcript": "hello",
                "done": true,
                "error": { "kind": "speechNotAllowed", "message": "denied" },
            })
        );
    }
}

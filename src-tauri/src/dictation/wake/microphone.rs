//! The wake word's microphone in the Windows app: the default input device through cpal
//! (WASAPI on Windows), downmixed to mono and handed to the worker ([`super::worker`]). It
//! uses nothing of Windows itself, so the same stream can be checked on other systems.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, ErrorKind, SampleFormat};

/// Mono audio from the microphone, at the device's rate.
pub struct Audio {
    /// Which opening of the microphone it came from; counting restarts at zero with each.
    pub generation: u32,
    pub rate: f64,
    pub samples: Vec<f32>,
}

static GENERATION: AtomicU32 = AtomicU32::new(0);

/// Averages the channels of interleaved frames into one.
fn downmix<T: Copy>(samples: &[T], channels: usize, to_f32: impl Fn(T) -> f32) -> Vec<f32> {
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().map(|&sample| to_f32(sample)).sum::<f32>() / channels as f32)
        .collect()
}

fn mono(data: &Data, channels: usize) -> Option<Vec<f32>> {
    match data.sample_format() {
        SampleFormat::F32 => data
            .as_slice::<f32>()
            .map(|samples| downmix(samples, channels, |sample| sample)),
        SampleFormat::I16 => data
            .as_slice::<i16>()
            .map(|samples| downmix(samples, channels, |sample| f32::from(sample) / 32_768.0)),
        SampleFormat::I32 => data
            .as_slice::<i32>()
            .map(|samples| downmix(samples, channels, |sample| sample as f32 / 2_147_483_648.0)),
        _ => None,
    }
}

/// The wake word's stream from the default input device, closed when dropped.
#[derive(Default)]
pub struct Microphone {
    stream: Option<cpal::Stream>,
    broken: Arc<AtomicBool>,
}

impl Microphone {
    /// Opens the default input device, anew if it was open, and sends its audio to `sender`.
    pub fn open(&mut self, sender: SyncSender<Audio>) -> Result<(), String> {
        self.close();
        self.start(sender)
    }

    fn start(&mut self, sender: SyncSender<Audio>) -> Result<(), String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| "No microphone was found.".to_owned())?;
        let supported = device
            .default_input_config()
            .map_err(|error| format!("The microphone cannot be used: {error}"))?;
        let format = supported.sample_format();
        if !matches!(
            format,
            SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32
        ) {
            return Err(format!(
                "The microphone's sample format ({format:?}) is not supported."
            ));
        }
        let config = supported.config();
        let channels = usize::from(config.channels.max(1));
        let rate = f64::from(config.sample_rate);
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let broken = Arc::new(AtomicBool::new(false));
        let on_error = {
            let broken = broken.clone();
            move |error: cpal::Error| match error.kind() {
                // The stream follows a new default device by itself, but at the old device's
                // rate; opening it again takes the new one's.
                ErrorKind::DeviceNotAvailable
                | ErrorKind::DeviceChanged
                | ErrorKind::StreamInvalidated => broken.store(true, Ordering::SeqCst),
                _ => eprintln!("toodo: the microphone reported: {error}"),
            }
        };
        let stream = device
            .build_input_stream_raw(
                config,
                format,
                move |data: &Data, _| {
                    if let Some(samples) = mono(data, channels) {
                        // A full queue means the detector fell behind; dropping a buffer
                        // beats blocking the audio thread.
                        let _ = sender.try_send(Audio {
                            generation,
                            rate,
                            samples,
                        });
                    }
                },
                on_error,
                None,
            )
            .map_err(|error| format!("The microphone did not start: {error}"))?;
        stream
            .play()
            .map_err(|error| format!("The microphone did not start: {error}"))?;
        self.stream = Some(stream);
        self.broken = broken;
        Ok(())
    }

    /// Whether the stream stopped or its device went away, so it has to be opened again.
    pub fn broken(&self) -> bool {
        self.broken.load(Ordering::SeqCst)
    }

    pub fn close(&mut self) {
        self.stream = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_averaged_into_one() {
        let stereo = [0.5f32, -0.5, 1.0, 0.0, 0.25, 0.75];
        assert_eq!(downmix(&stereo, 2, |s| s), vec![0.0, 0.5, 0.5]);
        assert_eq!(
            downmix(&[i16::MIN, 0, 16_384], 1, |s| f32::from(s) / 32_768.0),
            vec![-1.0, 0.0, 0.5]
        );
        // A frame cut off at the end of a buffer is dropped rather than misread.
        assert_eq!(downmix(&[1.0f32, 1.0, 1.0], 2, |s| s), vec![1.0]);
    }
}

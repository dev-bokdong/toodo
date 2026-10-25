//! The sherpa-onnx streaming Korean zipformer as the detector's [`Recognizer`], set up as
//! measured in docs/wake-word.md: one thread, modified beam search biased toward the wake
//! word (score 2.0), and an endpoint after 0.8 s of trailing silence.

use std::path::Path;

use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};

use super::detector::{Recognizer, SAMPLE_RATE};
use super::model;
use super::rule::Token;

/// The wake word "do-ri-ya", which the search is biased toward.
const HOTWORD: &str = "\u{b3c4}\u{b9ac}\u{c57c}\n";
const HOTWORD_SCORE: f32 = 2.0;

pub struct Sherpa(OnlineRecognizer);

impl Sherpa {
    /// Loads the model downloaded into `dir`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = |name: &str| Some(dir.join(name).to_string_lossy().into_owned());
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: path(model::ENCODER),
                    decoder: path(model::DECODER),
                    joiner: path(model::JOINER),
                },
                tokens: path(model::TOKENS),
                num_threads: 1,
                modeling_unit: Some("bpe".to_owned()),
                bpe_vocab: path(model::BPE_VOCAB),
                ..Default::default()
            },
            decoding_method: Some("modified_beam_search".to_owned()),
            max_active_paths: 4,
            enable_endpoint: true,
            rule1_min_trailing_silence: 2.4,
            rule2_min_trailing_silence: 0.8,
            rule3_min_utterance_length: 20.0,
            hotwords_buf: Some(HOTWORD.as_bytes().to_vec()),
            hotwords_score: HOTWORD_SCORE,
            ..Default::default()
        };
        OnlineRecognizer::create(&config)
            .map(Self)
            .ok_or_else(|| format!("cannot load the wake word model from {}", dir.display()))
    }
}

impl Recognizer for Sherpa {
    type Stream = OnlineStream;

    fn stream(&self) -> OnlineStream {
        self.0.create_stream()
    }

    fn accept(&self, stream: &mut OnlineStream, samples: &[f32]) {
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        while self.0.is_ready(stream) {
            self.0.decode(stream);
        }
    }

    fn tokens(&self, stream: &OnlineStream) -> Vec<Token> {
        let Some(result) = self.0.get_result(stream) else {
            return Vec::new();
        };
        let times = result.timestamps.unwrap_or_default();
        result.tokens.into_iter().zip(times).collect()
    }

    fn is_endpoint(&self, stream: &OnlineStream) -> bool {
        self.0.is_endpoint(stream)
    }

    fn reset(&self, stream: &mut OnlineStream) {
        self.0.reset(stream);
    }
}

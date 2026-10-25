//! The wake word detector over a streaming recognizer: it decodes 16 kHz audio, finds the
//! wake word in the transcript ([`rule::find_wake`]), and then keeps decoding the same
//! stream for the sentence after it. Nothing is handed to a second recognizer, so no
//! syllable is lost or repeated at the switch (docs/wake-word.md, "continue").

use super::rule::{self, Ending, Listening, Token};

/// The sample rate the recognizer takes.
pub const SAMPLE_RATE: u32 = 16_000;
/// Added to the last wake word token's start for its end (seconds).
const WAKE_TAIL: f32 = 0.08;
/// After the app stops speaking, detection waits this long before it listens again
/// (seconds), so the end of its own voice cannot wake it.
pub const RESUME_DELAY: f32 = 0.3;

/// A streaming recognizer the detector drives; the sherpa-onnx one in the app, a scripted
/// one in tests.
pub trait Recognizer {
    type Stream;
    fn stream(&self) -> Self::Stream;
    /// Takes 16 kHz mono samples and decodes what it can.
    fn accept(&self, stream: &mut Self::Stream, samples: &[f32]);
    /// The tokens since the stream started or was last reset, with their start times in
    /// seconds from that point.
    fn tokens(&self, stream: &Self::Stream) -> Vec<Token>;
    /// Whether the recognizer took the last stretch of silence for the end of an utterance.
    fn is_endpoint(&self, stream: &Self::Stream) -> bool;
    fn reset(&self, stream: &mut Self::Stream);
}

/// A wake word heard; times are seconds on the capture clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wake {
    pub start: f32,
    pub end: f32,
    pub detected_at: f32,
}

/// How the sentence after a wake word ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedBy {
    Quiet,
    Timeout,
    Cancel,
}

/// The sentence after a wake word. Only a non-empty message that ended by quiet is sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub message: String,
    pub wake: Wake,
    pub ended_by: EndedBy,
}

impl Outcome {
    pub fn sends(&self) -> bool {
        self.ended_by == EndedBy::Quiet && !self.message.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Wake(Wake),
    /// The sentence heard so far changed.
    Heard(String),
    Done(Outcome),
}

struct Hearing {
    wake: Wake,
    /// Tokens up to and including the wake word.
    skip: usize,
    listening: Listening,
    heard: String,
}

pub struct Detector<R: Recognizer> {
    recognizer: R,
    stream: R::Stream,
    /// The capture sample where the stream's clock starts.
    origin: u64,
    /// The capture sample after the last one fed.
    fed: u64,
    /// First tokens of wake words already reported in this utterance.
    seen: Vec<usize>,
    hearing: Option<Hearing>,
}

fn seconds(samples: u64) -> f32 {
    samples as f32 / SAMPLE_RATE as f32
}

impl<R: Recognizer> Detector<R> {
    pub fn new(recognizer: R) -> Self {
        let stream = recognizer.stream();
        Self {
            recognizer,
            stream,
            origin: 0,
            fed: 0,
            seen: Vec::new(),
            hearing: None,
        }
    }

    /// Starts over with a fresh stream whose clock begins at capture sample `at`: after a
    /// sentence, after a pause, and when capture restarts (at 0).
    pub fn restart(&mut self, at: u64) {
        self.stream = self.recognizer.stream();
        self.origin = at;
        self.fed = at;
        self.seen.clear();
        self.hearing = None;
    }

    pub fn is_hearing(&self) -> bool {
        self.hearing.is_some()
    }

    /// Ends the sentence being heard without sending it, and starts over.
    pub fn cancel(&mut self) -> Option<Outcome> {
        let hearing = self.hearing.take()?;
        let outcome = Outcome {
            message: hearing.heard,
            wake: hearing.wake,
            ended_by: EndedBy::Cancel,
        };
        self.restart(self.fed);
        Some(outcome)
    }

    /// Feeds the next chunk of capture audio.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<Event> {
        self.recognizer.accept(&mut self.stream, samples);
        self.fed += samples.len() as u64;
        let now = seconds(self.fed);
        let tokens = self.recognizer.tokens(&self.stream);
        let mut events = Vec::new();

        if self.hearing.is_none() {
            let base = seconds(self.origin);
            let found = rule::find_wake(&tokens)
                .into_iter()
                .find(|(first, _)| !self.seen.contains(first));
            if let Some((first, last)) = found {
                self.seen.push(first);
                let wake = Wake {
                    start: base + tokens[first].1,
                    end: base + tokens[last].1 + WAKE_TAIL,
                    detected_at: now,
                };
                self.hearing = Some(Hearing {
                    wake,
                    skip: last + 1,
                    listening: Listening::new(now),
                    heard: String::new(),
                });
                events.push(Event::Wake(wake));
            } else if self.recognizer.is_endpoint(&self.stream) {
                self.recognizer.reset(&mut self.stream);
                self.origin = self.fed;
                self.seen.clear();
                return events;
            }
        }

        let Some(hearing) = self.hearing.as_mut() else {
            return events;
        };
        let sentence = tokens.get(hearing.skip..).unwrap_or_default();
        let heard = rule::message(sentence);
        if heard != hearing.heard {
            hearing.heard.clone_from(&heard);
            events.push(Event::Heard(heard));
        }
        if let Some(ending) = hearing.listening.ended(sentence, now) {
            let outcome = Outcome {
                message: hearing.heard.clone(),
                wake: hearing.wake,
                ended_by: match ending {
                    Ending::Quiet => EndedBy::Quiet,
                    Ending::Timeout => EndedBy::Timeout,
                },
            };
            self.restart(self.fed);
            events.push(Event::Done(outcome));
        }
        events
    }
}

/// Why detection is held: while the app reads a reply aloud (and [`RESUME_DELAY`] after),
/// and while a dictated message takes the microphone.
#[derive(Debug, Default, Clone)]
pub struct Holds {
    speaking: bool,
    dictating: bool,
    /// Capture time (seconds) before which detection stays held after speaking stopped.
    until: f32,
}

impl Holds {
    pub fn set_speaking(&mut self, speaking: bool, now: f32) {
        if self.speaking && !speaking {
            self.until = now + RESUME_DELAY;
        }
        self.speaking = speaking;
    }

    pub fn set_dictating(&mut self, dictating: bool) {
        self.dictating = dictating;
    }

    /// Whether audio at capture time `now` goes to the detector.
    pub fn listens(&self, now: f32) -> bool {
        !self.speaking && !self.dictating && now >= self.until
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: usize = SAMPLE_RATE as usize / 10;
    const WAKE: &str = "\u{2581}\u{b3c4}\u{b9ac}\u{c57c}"; // do-ri-ya
    const ONEUL: &str = "\u{2581}\u{c624}\u{b298}"; // "today"
    const HAL: &str = "\u{2581}\u{d560}"; // "to do"

    /// Hears a script of `(token, at)` words: each token shows up once the stream has been
    /// fed past its time, and an endpoint comes `endpoint_after` seconds after the last one.
    /// A new stream starts where the audio fed so far ends, as a real one hears only what
    /// comes after it.
    struct Script {
        words: Vec<(&'static str, f32)>,
        endpoint_after: f32,
        fed: std::cell::Cell<f32>,
    }

    fn script(words: Vec<(&'static str, f32)>) -> Script {
        Script {
            words,
            endpoint_after: 0.8,
            fed: std::cell::Cell::new(0.0),
        }
    }

    /// The stream's position, in seconds of the script.
    struct Position {
        /// Where the stream's clock starts in the script.
        start: f32,
        at: f32,
    }

    impl Recognizer for &Script {
        type Stream = Position;

        fn stream(&self) -> Position {
            Position {
                start: self.fed.get(),
                at: self.fed.get(),
            }
        }

        fn accept(&self, stream: &mut Position, samples: &[f32]) {
            stream.at += samples.len() as f32 / SAMPLE_RATE as f32;
            self.fed.set(stream.at);
        }

        fn tokens(&self, stream: &Position) -> Vec<Token> {
            self.words
                .iter()
                .filter(|(_, at)| *at >= stream.start && *at < stream.at)
                .map(|&(word, at)| (word.to_owned(), at - stream.start))
                .collect()
        }

        fn is_endpoint(&self, stream: &Position) -> bool {
            let last = self
                .words
                .iter()
                .map(|(_, at)| *at)
                .filter(|at| *at >= stream.start && *at < stream.at)
                .fold(stream.start, f32::max);
            stream.at - last >= self.endpoint_after
        }

        fn reset(&self, stream: &mut Position) {
            stream.start = stream.at;
        }
    }

    /// Feeds `seconds` of audio in 100 ms chunks and collects the events.
    fn run<R: Recognizer>(detector: &mut Detector<R>, seconds: f32) -> Vec<Event> {
        let chunk = vec![0.0; CHUNK];
        (0..(seconds * 10.0).round() as usize)
            .flat_map(|_| detector.feed(&chunk))
            .collect()
    }

    fn outcomes(events: &[Event]) -> Vec<&Outcome> {
        events
            .iter()
            .filter_map(|event| match event {
                Event::Done(outcome) => Some(outcome),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_sentence_said_in_one_breath_after_the_wake_word_is_sent_once() {
        let script = script(vec![(WAKE, 1.0), (ONEUL, 1.35), (HAL, 1.7)]);
        let mut detector = Detector::new(&script);
        let events = run(&mut detector, 8.0);

        let wakes: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, Event::Wake(_)))
            .collect();
        assert_eq!(wakes.len(), 1);
        let done = outcomes(&events);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].message, "\u{c624}\u{b298} \u{d560}");
        assert_eq!(done[0].ended_by, EndedBy::Quiet);
        assert!(done[0].sends());
        assert!((done[0].wake.start - 1.0).abs() < 1e-3);
        // The first word of the sentence was heard while still listening for its end.
        assert!(events.contains(&Event::Heard("\u{c624}\u{b298}".to_owned())));
        assert!(!detector.is_hearing());
    }

    #[test]
    fn the_wake_word_alone_times_out_and_sends_nothing() {
        let script = script(vec![(WAKE, 1.0)]);
        let mut detector = Detector::new(&script);
        let done_events = run(&mut detector, 8.0);
        let done = outcomes(&done_events);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].ended_by, EndedBy::Timeout);
        assert!(!done[0].sends());
    }

    #[test]
    fn speech_without_the_wake_word_sends_nothing() {
        let script = script(vec![(ONEUL, 1.0), (HAL, 1.3)]);
        let mut detector = Detector::new(&script);
        assert!(run(&mut detector, 6.0).is_empty());
    }

    #[test]
    fn a_wake_word_in_a_new_utterance_after_an_endpoint_is_found() {
        let script = script(vec![(ONEUL, 0.5), (HAL, 0.7), (WAKE, 3.0), (ONEUL, 3.4)]);
        let mut detector = Detector::new(&script);
        let events = run(&mut detector, 9.0);
        let done = outcomes(&events);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].message, "\u{c624}\u{b298}");
        assert!((done[0].wake.start - 3.0).abs() < 1e-3);
    }

    #[test]
    fn cancel_ends_the_sentence_without_sending_and_listens_again() {
        let script = script(vec![(WAKE, 1.0), (ONEUL, 1.4)]);
        let mut detector = Detector::new(&script);
        run(&mut detector, 1.6);
        assert!(detector.is_hearing());
        let cancelled = detector.cancel().expect("was hearing");
        assert_eq!(cancelled.ended_by, EndedBy::Cancel);
        assert!(!cancelled.sends());
        assert!(!detector.is_hearing());
        assert_eq!(detector.cancel(), None);
    }

    #[test]
    fn detection_is_held_while_speaking_and_shortly_after() {
        let mut holds = Holds::default();
        assert!(holds.listens(0.0));
        holds.set_speaking(true, 1.0);
        assert!(!holds.listens(2.0));
        holds.set_speaking(false, 3.0);
        assert!(!holds.listens(3.2));
        assert!(holds.listens(3.3));
        holds.set_dictating(true);
        assert!(!holds.listens(9.0));
        holds.set_dictating(false);
        assert!(holds.listens(9.0));
    }
}

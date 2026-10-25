//! The wake word rule on a streaming recognizer's tokens, ported from the measured prototype
//! (`scripts/wake-word/wake.py`, see docs/wake-word.md): three syllables at the start of an
//! utterance whose middle syllable is spoken "ri" and which are within two jamo edits of
//! "do-ri-ya", matched by pronunciation rather than spelling.

/// Jamo of the wake word "do-ri-ya" as spoken.
const WAKE_JAMO: &str = "\u{3137}\u{3157}\u{3139}\u{3163}\u{3151}";
/// What the middle syllable of a match must start with: "ri".
const RI: &str = "\u{3139}\u{3163}";
/// The word piece marker of the recognizer's BPE tokens.
const WORD_MARK: char = '\u{2581}';

/// How many jamo edits a match may be away from the wake word.
pub const MAX_DISTANCE: usize = 2;
/// The spoken token before a wake word must start at least this long before it (seconds),
/// so a sentence ending in the same sound ("...do-ri-ya", "it is one's duty") does not wake.
pub const MIN_GAP: f32 = 0.5;
/// The sentence ends this long after its last new word (seconds).
pub const QUIET_SECONDS: f32 = 2.0;
/// With no word after the wake word, listening gives up after this long (seconds).
pub const START_TIMEOUT: f32 = 5.0;

const CHO: [char; 19] = [
    '\u{3131}', '\u{3132}', '\u{3134}', '\u{3137}', '\u{3138}', '\u{3139}', '\u{3141}', '\u{3142}',
    '\u{3143}', '\u{3145}', '\u{3146}', '\u{3147}', '\u{3148}', '\u{3149}', '\u{314a}', '\u{314b}',
    '\u{314c}', '\u{314d}', '\u{314e}',
];
const SILENT_INITIAL: char = '\u{3147}';
const JUNG: [char; 21] = [
    '\u{314f}', '\u{3150}', '\u{3151}', '\u{3152}', '\u{3153}', '\u{3154}', '\u{3155}', '\u{3156}',
    '\u{3157}', '\u{3158}', '\u{3159}', '\u{315a}', '\u{315b}', '\u{315c}', '\u{315d}', '\u{315e}',
    '\u{315f}', '\u{3160}', '\u{3161}', '\u{3162}', '\u{3163}',
];
const JONG: [Option<char>; 28] = [
    None,
    Some('\u{3131}'),
    Some('\u{3132}'),
    Some('\u{3133}'),
    Some('\u{3134}'),
    Some('\u{3135}'),
    Some('\u{3136}'),
    Some('\u{3137}'),
    Some('\u{3139}'),
    Some('\u{313a}'),
    Some('\u{313b}'),
    Some('\u{313c}'),
    Some('\u{313d}'),
    Some('\u{313e}'),
    Some('\u{313f}'),
    Some('\u{3140}'),
    Some('\u{3141}'),
    Some('\u{3142}'),
    Some('\u{3144}'),
    Some('\u{3145}'),
    Some('\u{3146}'),
    Some('\u{3147}'),
    Some('\u{3148}'),
    Some('\u{314a}'),
    Some('\u{314b}'),
    Some('\u{314c}'),
    Some('\u{314d}'),
    Some('\u{314e}'),
];

fn is_syllable(c: char) -> bool {
    ('\u{ac00}'..='\u{d7a3}').contains(&c)
}

/// A recognized token and when it started, in seconds.
pub type Token = (String, f32);

/// The jamo of each syllable as spoken: a final consonant before a silent initial moves
/// over to it ("dol-i-ya" is spoken "do-ri-ya"). Characters other than syllables are skipped.
pub fn pronounce(text: &str) -> Vec<String> {
    let mut syllables: Vec<[Option<char>; 3]> = text
        .chars()
        .filter(|&c| is_syllable(c))
        .map(|c| {
            let code = c as usize - 0xac00;
            [
                Some(CHO[code / 588]),
                Some(JUNG[code % 588 / 28]),
                JONG[code % 28],
            ]
        })
        .collect();
    for i in 1..syllables.len() {
        if let Some(last) = syllables[i - 1][2]
            && syllables[i][0] == Some(SILENT_INITIAL)
        {
            syllables[i - 1][2] = None;
            syllables[i][0] = Some(last);
        }
    }
    syllables
        .into_iter()
        .map(|[initial, vowel, last]| {
            initial
                .filter(|&c| c != SILENT_INITIAL)
                .into_iter()
                .chain(vowel)
                .chain(last)
                .collect()
        })
        .collect()
}

/// The edit distance between two strings, counted in characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current.push((previous[j + 1] + 1).min(current[j] + 1).min(substitute));
        }
        previous = current;
    }
    previous[b.len()]
}

/// `(first, last, distance)` token indexes of every three syllables whose middle one is
/// spoken "ri" and which are at most `max_distance` jamo edits from the wake word.
fn wake_windows(tokens: &[Token], max_distance: usize) -> Vec<(usize, usize, usize)> {
    let (chars, owners): (Vec<char>, Vec<usize>) = tokens
        .iter()
        .enumerate()
        .flat_map(|(i, (token, _))| {
            token
                .chars()
                .filter(|&c| c != WORD_MARK && is_syllable(c))
                .map(move |c| (c, i))
        })
        .unzip();
    let mut windows = Vec::new();
    for i in 0..chars.len().saturating_sub(2) {
        let spoken: String = chars[i..i + 3].iter().collect();
        let spoken = pronounce(&spoken);
        if spoken[1].starts_with(RI) {
            let edits = distance(&spoken.concat(), WAKE_JAMO);
            if edits <= max_distance {
                windows.push((owners[i], owners[i + 2], edits));
            }
        }
    }
    windows
}

/// `(first, last)` token indexes of the wake words in `tokens`: matches of
/// [`MAX_DISTANCE`] edits or fewer whose previous spoken token started [`MIN_GAP`] or more
/// earlier.
pub fn find_wake(tokens: &[Token]) -> Vec<(usize, usize)> {
    let spoken: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, (token, _))| token.chars().any(is_syllable))
        .map(|(i, _)| i)
        .collect();
    let mut found: Vec<(usize, usize)> = Vec::new();
    for (first, last, _) in wake_windows(tokens, MAX_DISTANCE) {
        if !found.iter().any(|&(seen, _)| seen == first) {
            found.push((first, last));
        }
    }
    found.retain(|&(first, _)| {
        let Some(k) = spoken.iter().position(|&i| i == first) else {
            return false;
        };
        k == 0 || tokens[first].1 - tokens[spoken[k - 1]].1 >= MIN_GAP
    });
    found.sort_unstable();
    found
}

/// Whether a token holds a word rather than only punctuation or the word marker.
fn is_spoken(token: &str) -> bool {
    token.chars().any(char::is_alphanumeric)
}

/// The message in the tokens after a wake word: word marks become spaces, and surrounding
/// spaces and sentence punctuation go.
pub fn message(tokens: &[Token]) -> String {
    let text: String = tokens.iter().map(|(token, _)| token.as_str()).collect();
    text.replace(WORD_MARK, " ")
        .trim_matches(|c: char| matches!(c, ' ' | ',' | '.' | '?' | '!'))
        .to_owned()
}

/// Whether the sentence after a wake word has ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// Words followed and then [`QUIET_SECONDS`] passed without a new one.
    Quiet,
    /// No word followed within [`START_TIMEOUT`].
    Timeout,
}

/// Watches the sentence after a wake word for its end.
#[derive(Debug, Clone)]
pub struct Listening {
    began: f32,
    last_change: f32,
    words: usize,
}

impl Listening {
    /// Starts watching at `now`, in seconds on the capture clock.
    pub fn new(now: f32) -> Self {
        Self {
            began: now,
            last_change: now,
            words: 0,
        }
    }

    /// Given the tokens after the wake word at time `now`, whether the sentence has ended.
    pub fn ended(&mut self, sentence: &[Token], now: f32) -> Option<Ending> {
        let words = sentence
            .iter()
            .filter(|(token, _)| is_spoken(token))
            .count();
        if words != self.words {
            self.words = words;
            self.last_change = now;
        }
        if self.words == 0 {
            (now - self.began >= START_TIMEOUT).then_some(Ending::Timeout)
        } else {
            (now - self.last_change >= QUIET_SECONDS).then_some(Ending::Quiet)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hangul in tests is written as escapes, since tracked files stay English.
    const DORIYA: &str = "\u{b3c4}\u{b9ac}\u{c57c}"; // do-ri-ya
    const DOL_IYA: &str = "\u{b3cc}\u{c774}\u{c57c}"; // dol-i-ya, spoken as do-ri-ya
    const DUL_IYA: &str = "\u{b458}\u{c774}\u{c57c}"; // dul-i-ya, one edit away
    const TORIYA: &str = "\u{d1a0}\u{b9ac}\u{c57c}"; // to-ri-ya, a rhyming name
    const DOSIUI: &str = "\u{b3c4}\u{c2dc}\u{c758}"; // do-si-ui, no "ri" in the middle
    const ONEUL: &str = "\u{c624}\u{b298}"; // "today"
    const HAL: &str = "\u{d560}"; // "to do"
    const IL: &str = "\u{c77c}"; // "work"

    fn tokens(words: &[(&str, f32)]) -> Vec<Token> {
        words
            .iter()
            .map(|&(word, at)| (word.to_owned(), at))
            .collect()
    }

    #[test]
    fn a_final_consonant_moves_onto_a_silent_initial() {
        assert_eq!(pronounce(DOL_IYA), pronounce(DORIYA));
        assert_eq!(pronounce(DORIYA).concat(), WAKE_JAMO);
        assert!(pronounce("a b").is_empty());
    }

    #[test]
    fn edit_distance_counts_characters() {
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance(WAKE_JAMO, WAKE_JAMO), 0);
    }

    #[test]
    fn the_wake_word_matches_by_sound_at_the_start() {
        for word in [DORIYA, DOL_IYA, DUL_IYA, TORIYA] {
            let heard = tokens(&[(&format!("\u{2581}{word}"), 0.2), ("\u{2581}x", 0.9)]);
            assert_eq!(find_wake(&heard), vec![(0, 0)], "{word}");
        }
        let heard = tokens(&[(&format!("\u{2581}{DOSIUI}"), 0.2)]);
        assert!(find_wake(&heard).is_empty());
    }

    #[test]
    fn a_wake_word_split_over_tokens_reports_its_first_and_last_token() {
        let heard = tokens(&[
            ("\u{2581}\u{b3c4}", 0.3),
            ("\u{b9ac}", 0.4),
            ("\u{c57c}", 0.5),
            (&format!("\u{2581}{ONEUL}"), 0.7),
        ]);
        assert_eq!(find_wake(&heard), vec![(0, 2)]);
    }

    #[test]
    fn the_wake_word_right_after_another_word_does_not_count() {
        // "...work do-ri-ya" ends a sentence with the wake word's sound.
        let close = tokens(&[(&format!("\u{2581}{IL}"), 1.0), (DORIYA, 1.3)]);
        assert!(find_wake(&close).is_empty());
        let apart = tokens(&[(&format!("\u{2581}{IL}"), 1.0), (DORIYA, 1.5)]);
        assert_eq!(find_wake(&apart), vec![(1, 1)]);
    }

    #[test]
    fn the_message_is_the_words_without_marks_or_end_punctuation() {
        let sentence = tokens(&[
            (&format!("\u{2581}{ONEUL}"), 1.0),
            (&format!("\u{2581}{HAL}"), 1.3),
            (&format!("\u{2581}{IL}"), 1.5),
            ("?", 1.6),
        ]);
        assert_eq!(message(&sentence), format!("{ONEUL} {HAL} {IL}"));
        assert_eq!(message(&[]), "");
    }

    #[test]
    fn listening_ends_after_quiet_or_without_words() {
        let mut listening = Listening::new(10.0);
        assert_eq!(listening.ended(&[], 14.9), None);
        assert_eq!(listening.ended(&[], 15.0), Some(Ending::Timeout));

        let mut listening = Listening::new(10.0);
        let one = tokens(&[(ONEUL, 10.2)]);
        let two = tokens(&[(ONEUL, 10.2), (HAL, 11.0)]);
        assert_eq!(listening.ended(&one, 10.4), None);
        assert_eq!(listening.ended(&two, 11.2), None);
        assert_eq!(listening.ended(&two, 13.1), None);
        assert_eq!(listening.ended(&two, 13.2), Some(Ending::Quiet));
        // Punctuation alone is not a new word.
        let punctuated = tokens(&[(ONEUL, 10.2), (HAL, 11.0), ("?", 11.1)]);
        assert_eq!(listening.ended(&punctuated, 13.3), Some(Ending::Quiet));
    }
}

//! The Korean streaming recognizer the wake word runs on. Its files are downloaded from the
//! upstream model repository the first time the wake word is turned on, pinned to one
//! revision and checked against their SHA-256, rather than shipped in the app (see the
//! Licenses section of docs/wake-word.md).

use std::fs;
use std::path::Path;

pub struct ModelFile {
    pub name: &'static str,
    pub size: u64,
    sha256: &'static str,
}

impl ModelFile {
    /// Whether `size` bytes whose SHA-256 is `digest` are this file.
    pub fn matches(&self, size: u64, digest: &[u8]) -> bool {
        size == self.size && hex(digest) == self.sha256
    }
}

pub const ENCODER: &str = "encoder-epoch-99-avg-1.int8.onnx";
pub const DECODER: &str = "decoder-epoch-99-avg-1.onnx";
pub const JOINER: &str = "joiner-epoch-99-avg-1.int8.onnx";
pub const TOKENS: &str = "tokens.txt";
pub const BPE_MODEL: &str = "bpe.model";
/// The model's word pieces with their scores, one `piece<TAB>score` line each, which the
/// recognizer needs to turn the wake word into tokens for biasing its search.
pub const BPE_VOCAB: &str = "bpe.vocab";

pub const FILES: [ModelFile; 5] = [
    ModelFile {
        name: ENCODER,
        size: 126_968_852,
        sha256: "8d0b1aa24fbedd4e3948564ab7facd151b8ce9b0c48fc987c541de2de3af5697",
    },
    ModelFile {
        name: DECODER,
        size: 11_309_084,
        sha256: "b29cfb4575141e50a30a22b2c4579934f3d4f45b83c9c8c08c3aef5a3fa7abfc",
    },
    ModelFile {
        name: JOINER,
        size: 2_581_421,
        sha256: "128b80a66a1f718488af8560f9d15895109b99ff3e573f0a0130e03774ef1ced",
    },
    ModelFile {
        name: TOKENS,
        size: 60_246,
        sha256: "016bdf0965029263b7ad01b742366ee542ef0bef38261510e8176ff6f2e9e668",
    },
    ModelFile {
        name: BPE_MODEL,
        size: 314_212,
        sha256: "1491bc92c47dfda4225f5b8930fba3cfa34c3b1ccd25e7d96c630a262f3e918d",
    },
];

/// Whether `dir` holds the whole model: every file is renamed into place only after its
/// checksum matched, so a file of the right size is the checked one.
pub fn is_ready(dir: &Path) -> bool {
    FILES
        .iter()
        .all(|file| fs::metadata(dir.join(file.name)).is_ok_and(|meta| meta.len() == file.size))
        && dir.join(BPE_VOCAB).is_file()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `piece<TAB>score` lines of a SentencePiece model (`bpe.model`, a `ModelProto`
/// message): its field 1 repeats `SentencePiece { piece = 1 (string), score = 2 (float) }`.
pub fn bpe_vocab(model: &[u8]) -> Result<String, String> {
    let mut vocab = String::new();
    for field in Fields(model) {
        if let (1, Value::Bytes(piece)) = field? {
            let mut text = None;
            let mut score = 0.0f32;
            for inner in Fields(piece) {
                match inner? {
                    (1, Value::Bytes(bytes)) => {
                        text = Some(std::str::from_utf8(bytes).map_err(|error| error.to_string())?);
                    }
                    (2, Value::Fixed32(bits)) => score = f32::from_bits(bits),
                    _ => {}
                }
            }
            let text = text.ok_or("a piece has no text")?;
            vocab.push_str(&format!("{text}\t{score}\n"));
        }
    }
    if vocab.is_empty() {
        return Err("no pieces".to_owned());
    }
    Ok(vocab)
}

enum Value<'a> {
    Varint,
    Fixed64,
    Bytes(&'a [u8]),
    Fixed32(u32),
}

/// The `(field number, value)` pairs of a protobuf message.
struct Fields<'a>(&'a [u8]);

fn varint(bytes: &mut &[u8]) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or("truncated varint")?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("varint too long".to_owned())
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
    if bytes.len() < n {
        return Err("truncated field".to_owned());
    }
    let (head, rest) = bytes.split_at(n);
    *bytes = rest;
    Ok(head)
}

impl<'a> Iterator for Fields<'a> {
    type Item = Result<(u64, Value<'a>), String>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let mut read = || -> Result<(u64, Value<'a>), String> {
            let key = varint(&mut self.0)?;
            let value = match key & 7 {
                0 => varint(&mut self.0).map(|_| Value::Varint)?,
                1 => take(&mut self.0, 8).map(|_| Value::Fixed64)?,
                2 => {
                    let len =
                        usize::try_from(varint(&mut self.0)?).map_err(|error| error.to_string())?;
                    Value::Bytes(take(&mut self.0, len)?)
                }
                5 => {
                    let raw = take(&mut self.0, 4)?;
                    Value::Fixed32(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
                }
                other => return Err(format!("unsupported wire type {other}")),
            };
            Ok((key >> 3, value))
        };
        let item = read();
        if item.is_err() {
            self.0 = &[];
        }
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn a_file_matches_only_with_its_size_and_checksum() {
        let file = ModelFile {
            name: "abc",
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        assert!(file.matches(3, &Sha256::digest(b"abc")));
        assert!(!file.matches(4, &Sha256::digest(b"abc")));
        assert!(!file.matches(3, &Sha256::digest(b"abd")));
    }

    fn piece(text: &str, score: f32, kind: u8) -> Vec<u8> {
        let mut message = vec![0x0a, text.len() as u8];
        message.extend(text.as_bytes());
        message.push(0x15);
        message.extend(score.to_le_bytes());
        message.extend([0x18, kind]);
        message
    }

    #[test]
    fn the_vocab_lists_every_piece_with_its_score() {
        let mut model = Vec::new();
        for (text, score) in [
            ("<unk>", 0.0),
            ("\u{2581}\u{b3c4}", -12.5),
            ("\u{b9ac}", -3.25),
        ] {
            let message = piece(text, score, 1);
            model.push(0x0a);
            model.push(message.len() as u8);
            model.extend(message);
        }
        // Other fields of ModelProto (here trainer_spec = 2) are skipped.
        model.extend([0x12, 0x02, 0x08, 0x01]);
        assert_eq!(
            bpe_vocab(&model).unwrap(),
            "<unk>\t0\n\u{2581}\u{b3c4}\t-12.5\n\u{b9ac}\t-3.25\n"
        );
    }

    #[test]
    fn a_broken_model_is_rejected() {
        assert!(bpe_vocab(&[]).is_err());
        assert!(bpe_vocab(&[0x0a, 0x05, 0x0a]).is_err());
        assert!(bpe_vocab(&[0x0b]).is_err());
    }

    #[test]
    fn a_model_is_ready_only_with_every_file_at_its_size() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_ready(dir.path()));
        for file in &FILES {
            let out = fs::File::create(dir.path().join(file.name)).unwrap();
            out.set_len(file.size).unwrap();
        }
        assert!(!is_ready(dir.path()));
        fs::write(dir.path().join(BPE_VOCAB), "x\t0\n").unwrap();
        assert!(is_ready(dir.path()));
        fs::File::create(dir.path().join(TOKENS))
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(!is_ready(dir.path()));
    }
}

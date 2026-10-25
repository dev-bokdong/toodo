//! Downloads the wake word's model (see [`super::model`]) into the app data directory.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::model::{self, BPE_MODEL, BPE_VOCAB, FILES, ModelFile};

/// The model's upstream repository, at the revision whose files [`FILES`] lists.
const BASE_URL: &str = "https://huggingface.co/k2-fsa/sherpa-onnx-streaming-zipformer-korean-2024-06-16/resolve/ba6078bca4daf3f0dd37f79d0ab505af71df14a6";

/// The bytes of every model file together.
pub fn total_size() -> u64 {
    FILES.iter().map(|file| file.size).sum()
}

#[derive(Debug)]
pub enum DownloadError {
    Http(String),
    Cancelled,
    Checksum(&'static str),
    Io(io::Error),
    InvalidModel(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(message) => write!(f, "the download failed: {message}"),
            Self::Cancelled => write!(f, "the download was cancelled"),
            Self::Checksum(name) => write!(f, "{name} did not match its checksum"),
            Self::Io(error) => write!(f, "cannot write the model: {error}"),
            Self::InvalidModel(message) => write!(f, "the model is not usable: {message}"),
        }
    }
}

impl From<io::Error> for DownloadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Downloads the missing model files into `dir`, calling `progress(done, total)` in bytes
/// as they come in, then writes [`BPE_VOCAB`]. Files already in place are kept. The
/// download stops when `progress` returns false.
pub async fn download(
    dir: &Path,
    progress: impl Fn(u64, u64) -> bool,
) -> Result<(), DownloadError> {
    fs::create_dir_all(dir)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| DownloadError::Http(error.to_string()))?;
    let total = total_size();
    let mut done = 0;
    for file in &FILES {
        let target = dir.join(file.name);
        if fs::metadata(&target).is_ok_and(|meta| meta.len() == file.size) {
            done += file.size;
            if !progress(done, total) {
                return Err(DownloadError::Cancelled);
            }
            continue;
        }
        let partial = part_path(&target);
        let result = fetch(&client, file, &partial, |bytes| {
            progress(done + bytes, total)
        })
        .await;
        if let Err(error) = result {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        fs::rename(&partial, &target)?;
        done += file.size;
    }
    let vocab =
        model::bpe_vocab(&fs::read(dir.join(BPE_MODEL))?).map_err(DownloadError::InvalidModel)?;
    let partial = part_path(&dir.join(BPE_VOCAB));
    fs::write(&partial, vocab)?;
    fs::rename(&partial, dir.join(BPE_VOCAB))?;
    Ok(())
}

fn part_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(".part");
    target.with_file_name(name)
}

async fn fetch(
    client: &reqwest::Client,
    file: &ModelFile,
    partial: &Path,
    progress: impl Fn(u64) -> bool,
) -> Result<(), DownloadError> {
    let http = |error: reqwest::Error| DownloadError::Http(error.to_string());
    let mut response = client
        .get(format!("{BASE_URL}/{}", file.name))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(http)?;
    let mut out = io::BufWriter::new(fs::File::create(partial)?);
    let mut hasher = Sha256::new();
    let mut bytes = 0;
    while let Some(chunk) = response.chunk().await.map_err(http)? {
        hasher.update(&chunk);
        out.write_all(&chunk)?;
        bytes += chunk.len() as u64;
        if !progress(bytes.min(file.size)) {
            return Err(DownloadError::Cancelled);
        }
    }
    out.flush()?;
    if !file.matches(bytes, &hasher.finalize()) {
        return Err(DownloadError::Checksum(file.name));
    }
    Ok(())
}

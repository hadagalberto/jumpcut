//! Streaming PCM decoding through a pipe from `ffmpeg`.
//!
//! ffmpeg writes native-endian-independent `f32le` samples to stdout; jumpcut
//! reads them in bounded chunks, so analysing a multi-hour file needs constant
//! memory. No FFI and no temporary PCM file are involved.

use std::ffi::OsStr;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::{self, JoinHandle};

use thiserror::Error;

use crate::tool;

/// Decoding parameters for [`PcmDecoder`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeOptions {
    /// Output sample rate in Hz (16000 by default).
    pub sample_rate: u32,
    /// Output channel count (1 by default).
    pub channels: u16,
    /// Zero-based audio stream index; `None` selects the first audio stream.
    pub audio_stream: Option<u32>,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            channels: 1,
            audio_stream: None,
        }
    }
}

impl DecodeOptions {
    fn map_argument(self) -> String {
        self.audio_stream
            .map_or_else(|| "0:a:0".to_owned(), |index| format!("0:a:{index}"))
    }
}

/// Errors produced while spawning or reading the ffmpeg decoder.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// The requested input file does not exist.
    #[error("input file not found: `{0}`")]
    InputNotFound(PathBuf),
    /// ffmpeg could not be found on `PATH`.
    #[error("{}", tool::missing_message(tool))]
    ToolMissing {
        /// Name of the missing executable.
        tool: String,
    },
    /// ffmpeg could not be started.
    #[error("failed to spawn `{tool}`: {source}")]
    Spawn {
        /// Name of the executable.
        tool: String,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// Reading the PCM pipe failed.
    #[error("failed to read decoded PCM: {0}")]
    Read(#[from] io::Error),
    /// ffmpeg produced a partial 32-bit sample.
    #[error("ffmpeg produced a truncated 32-bit float sample")]
    TruncatedPcm,
    /// ffmpeg exited with a failure status.
    #[error("`{tool}` failed with {status}: {stderr}")]
    ProcessFailed {
        /// Name of the executable.
        tool: String,
        /// Exit status as reported by the OS.
        status: String,
        /// Captured standard error.
        stderr: String,
    },
    /// The decoder was already consumed by `finish`.
    #[error("PCM decoder has already been finished")]
    AlreadyFinished,
    /// A chunk size overflowed the byte count.
    #[error("requested PCM chunk size is too large")]
    ChunkTooLarge,
}

/// A live ffmpeg process streaming mono/stereo `f32` samples on stdout.
#[derive(Debug)]
pub struct PcmDecoder {
    child: Child,
    stdout: Option<BufReader<ChildStdout>>,
    stderr_thread: Option<JoinHandle<String>>,
    options: DecodeOptions,
    finished: bool,
}

impl PcmDecoder {
    /// Spawns `ffmpeg` to decode `input` into raw `f32le` PCM.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::ToolMissing`] with installation guidance when
    /// ffmpeg is absent, [`DecodeError::InputNotFound`] for a missing file, or
    /// [`DecodeError::ProcessFailed`] if ffmpeg cannot decode the input.
    pub fn spawn(input: &Path, options: DecodeOptions) -> Result<Self, DecodeError> {
        Self::spawn_with(OsStr::new("ffmpeg"), input, options)
    }

    fn spawn_with(tool: &OsStr, input: &Path, options: DecodeOptions) -> Result<Self, DecodeError> {
        let tool_name = tool.to_string_lossy().into_owned();
        if !input.is_file() {
            return Err(DecodeError::InputNotFound(input.to_path_buf()));
        }
        if tool::find(&tool_name).is_none() {
            return Err(DecodeError::ToolMissing { tool: tool_name });
        }

        let map = options.map_argument();
        let channels = options.channels.to_string();
        let sample_rate = options.sample_rate.to_string();
        let mut child = Command::new(tool)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-i"])
            .arg(input)
            .args([
                "-map",
                map.as_str(),
                "-vn",
                "-sn",
                "-dn",
                "-ac",
                channels.as_str(),
                "-ar",
                sample_rate.as_str(),
                "-f",
                "f32le",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| match source.kind() {
                io::ErrorKind::NotFound => DecodeError::ToolMissing { tool: tool_name },
                _ => DecodeError::Spawn {
                    tool: tool_name,
                    source,
                },
            })?;

        let stdout = BufReader::new(child.stdout.take().expect("stdout was piped"));
        let mut stderr = child.stderr.take().expect("stderr was piped");
        // Draining stderr in a separate thread avoids a deadlock when ffmpeg
        // writes more than a pipe buffer worth of diagnostics.
        let stderr_thread = thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });

        Ok(Self {
            child,
            stdout: Some(stdout),
            stderr_thread: Some(stderr_thread),
            options,
            finished: false,
        })
    }

    /// Sampling rate of the emitted PCM, in Hz.
    pub const fn sample_rate(&self) -> u32 {
        self.options.sample_rate
    }

    /// Channel count of the emitted PCM.
    pub const fn channels(&self) -> u16 {
        self.options.channels
    }

    /// Reads at most `max_samples` floats from the pipe.
    ///
    /// An empty vector means ffmpeg has finished writing; call [`Self::finish`]
    /// afterwards to check the exit status.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::Read`] on pipe failure,
    /// [`DecodeError::TruncatedPcm`] on a partial sample, or
    /// [`DecodeError::AlreadyFinished`] if the decoder was consumed.
    pub fn read_chunk(&mut self, max_samples: usize) -> Result<Vec<f32>, DecodeError> {
        if self.finished {
            return Err(DecodeError::AlreadyFinished);
        }
        if max_samples == 0 {
            return Ok(Vec::new());
        }
        let capacity = max_samples
            .checked_mul(4)
            .ok_or(DecodeError::ChunkTooLarge)?;
        let reader = self.stdout.as_mut().ok_or(DecodeError::AlreadyFinished)?;
        let mut bytes = vec![0_u8; capacity];
        let mut filled = 0;
        while filled < capacity {
            let read = reader.read(&mut bytes[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled % 4 != 0 {
            return Err(DecodeError::TruncatedPcm);
        }
        Ok(bytes[..filled]
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    /// Closes the pipe, waits for ffmpeg and verifies a successful exit.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::ProcessFailed`] with ffmpeg's stderr when the
    /// decoder exited unsuccessfully.
    pub fn finish(mut self) -> Result<(), DecodeError> {
        self.finished = true;
        self.stdout.take();
        let status = self.child.wait()?;
        let stderr = self.join_stderr();
        if status.success() {
            Ok(())
        } else {
            Err(DecodeError::ProcessFailed {
                tool: "ffmpeg".to_owned(),
                status: status.to_string(),
                stderr,
            })
        }
    }

    fn join_stderr(&mut self) -> String {
        self.stderr_thread
            .take()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    }

    /// Reads every sample and verifies the ffmpeg exit status.
    ///
    /// # Errors
    ///
    /// Propagates any [`DecodeError`] from reading or from the child process.
    pub fn read_all(mut self) -> Result<Vec<f32>, DecodeError> {
        let mut samples = Vec::new();
        loop {
            let chunk = self.read_chunk(8_192)?;
            if chunk.is_empty() {
                break;
            }
            samples.extend_from_slice(&chunk);
        }
        self.finish()?;
        Ok(samples)
    }
}

impl Drop for PcmDecoder {
    fn drop(&mut self) {
        self.finished = true;
        self.stdout.take();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = self.join_stderr();
    }
}

/// Convenience wrapper around [`PcmDecoder::spawn`] and
/// [`PcmDecoder::read_all`].
///
/// # Errors
///
/// Propagates any [`DecodeError`].
pub fn decode_all(input: &Path, options: DecodeOptions) -> Result<Vec<f32>, DecodeError> {
    PcmDecoder::spawn(input, options)?.read_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ffmpeg_available() -> bool {
        tool::is_available("ffmpeg") && tool::is_available("ffprobe")
    }

    fn make_wav(directory: &Path, duration: f64) -> PathBuf {
        let wav = directory.join("tone.wav");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency=440:sample_rate=16000:duration={duration}"),
                "-ac",
                "1",
                "-y",
            ])
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg failed to generate the test WAV");
        wav
    }

    #[test]
    fn default_options_match_spec() {
        let options = DecodeOptions::default();
        assert_eq!(options.sample_rate, 16_000);
        assert_eq!(options.channels, 1);
        assert_eq!(options.audio_stream, None);
        assert_eq!(options.map_argument(), "0:a:0");
        assert_eq!(
            DecodeOptions {
                audio_stream: Some(2),
                ..options
            }
            .map_argument(),
            "0:a:2"
        );
    }

    #[test]
    fn missing_ffmpeg_reports_installation_guidance() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.wav");
        std::fs::write(&input, b"dummy").unwrap();
        let error = PcmDecoder::spawn_with(
            OsStr::new("jumpcut-ffmpeg-absent-12345"),
            &input,
            DecodeOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, DecodeError::ToolMissing { .. }));
        let message = error.to_string();
        assert!(message.contains("ffmpeg"));
        assert!(message.contains("PATH"));
    }

    #[test]
    fn missing_input_is_reported_before_spawn() {
        let error = PcmDecoder::spawn(
            Path::new("jumpcut-no-such-audio-12345.wav"),
            DecodeOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(error, DecodeError::InputNotFound(_)));
    }

    #[test]
    #[allow(
        clippy::cast_precision_loss,
        reason = "converting a test sample count to f32 for RMS is safe here"
    )]
    fn decodes_exact_sample_count_and_content() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let wav = make_wav(directory.path(), 1.0);
        let samples = decode_all(&wav, DecodeOptions::default()).unwrap();
        assert!(
            samples.len().abs_diff(16_000) <= 32,
            "decoded {} samples",
            samples.len()
        );
        assert!(samples.iter().all(|sample| sample.is_finite()));
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        assert!((0.05..0.5).contains(&rms), "RMS was {rms}");
    }

    #[test]
    fn streams_in_bounded_chunks() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let wav = make_wav(directory.path(), 0.5);
        let mut decoder = PcmDecoder::spawn(&wav, DecodeOptions::default()).unwrap();
        assert_eq!(decoder.sample_rate(), 16_000);
        assert_eq!(decoder.channels(), 1);
        let mut total = 0;
        let mut chunks = 0;
        loop {
            let chunk = decoder.read_chunk(500).unwrap();
            assert!(chunk.len() <= 500);
            if chunk.is_empty() {
                break;
            }
            total += chunk.len();
            chunks += 1;
        }
        decoder.finish().unwrap();
        assert!(chunks > 10, "expected true streaming, got {chunks} chunks");
        assert!(total.abs_diff(8_000) <= 32, "decoded {total} samples");
    }

    #[test]
    fn decode_failure_includes_ffmpeg_stderr() {
        if !ffmpeg_available() {
            eprintln!("skipping: ffmpeg not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let wav = make_wav(directory.path(), 0.2);
        let error = PcmDecoder::spawn(
            &wav,
            DecodeOptions {
                audio_stream: Some(7),
                ..DecodeOptions::default()
            },
        )
        .unwrap()
        .read_all()
        .unwrap_err();
        assert!(matches!(error, DecodeError::ProcessFailed { .. }));
        let message = error.to_string();
        assert!(message.contains("matches no streams"), "{message}");
    }
}

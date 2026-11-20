//! Reading media metadata from `ffprobe` as JSON.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::rational::{FrameRate, Rational};
use crate::tool;

/// Errors produced while probing a media file.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// The requested input file does not exist.
    #[error("input file not found: `{0}`")]
    InputNotFound(PathBuf),
    /// ffprobe could not be found on `PATH`.
    #[error("{}", tool::missing_message(tool))]
    ToolMissing {
        /// Name of the missing executable.
        tool: String,
    },
    /// ffprobe could not be started.
    #[error("failed to spawn `{tool}`: {source}")]
    Spawn {
        /// Name of the executable.
        tool: String,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// ffprobe exited with a failure status.
    #[error("`{tool}` failed with {status}: {stderr}")]
    CommandFailed {
        /// Name of the executable.
        tool: String,
        /// Exit status as reported by the OS.
        status: String,
        /// Captured standard error.
        stderr: String,
    },
    /// The JSON emitted by ffprobe could not be decoded.
    #[error("could not parse ffprobe output: {0}")]
    Json(#[from] serde_json::Error),
    /// A metadata field had an unexpected value.
    #[error("invalid ffprobe metadata: {0}")]
    InvalidMetadata(String),
}

/// Metadata for one video stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VideoStream {
    /// ffprobe stream index.
    pub index: u32,
    /// Video codec name.
    pub codec: String,
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
    /// Exact frame rate when reported.
    pub frame_rate: Option<FrameRate>,
    /// Stream duration in seconds.
    pub duration: Option<Rational>,
    /// Number of frames, when ffprobe knows it.
    pub nb_frames: Option<u64>,
    /// ffprobe colour-space tag (for example `bt709`).
    pub color_space: Option<String>,
    /// ffprobe colour-primaries tag (for example `bt709`).
    pub color_primaries: Option<String>,
    /// ffprobe colour-transfer tag (for example `bt709`).
    pub color_transfer: Option<String>,
    /// ffprobe field order (`progressive`, `tt`, `bb`, `tb` or `bt`).
    pub field_order: Option<String>,
}

/// Metadata for one audio stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AudioStream {
    /// ffprobe stream index.
    pub index: u32,
    /// Audio codec name.
    pub codec: String,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Stream duration in seconds.
    pub duration: Option<Rational>,
}

/// Summary of a media file, using exact rational durations and frame rates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MediaInfo {
    /// Path that was probed.
    pub path: PathBuf,
    /// Container duration in seconds.
    pub duration: Option<Rational>,
    /// ffprobe container format name.
    pub format_name: Option<String>,
    /// First video stream, if any.
    pub video: Option<VideoStream>,
    /// All audio streams, in ffprobe order.
    pub audio: Vec<AudioStream>,
}

impl MediaInfo {
    /// The first audio stream, if the file contains one.
    pub fn primary_audio_stream(&self) -> Option<&AudioStream> {
        self.audio.first()
    }

    /// Container duration as `f64`, for display only.
    pub fn duration_seconds(&self) -> Option<f64> {
        self.duration.map(Rational::to_f64)
    }

    /// Effective frame rate: the reported rate, or 30 fps as a conservative
    /// fallback for still/image inputs.
    pub fn frame_rate_or_default(&self) -> FrameRate {
        self.video
            .as_ref()
            .and_then(|video| video.frame_rate)
            .unwrap_or(FrameRate::FPS_30)
    }
}

#[derive(Debug, Deserialize)]
struct RawProbe {
    #[serde(default)]
    format: RawFormat,
    #[serde(default)]
    streams: Vec<RawStream>,
}

#[derive(Debug, Default, Deserialize)]
struct RawFormat {
    duration: Option<String>,
    format_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawStream {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
    duration: Option<String>,
    nb_frames: Option<String>,
    color_space: Option<String>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    field_order: Option<String>,
}

fn parse_duration(raw: Option<&String>) -> Option<Rational> {
    raw.and_then(|text| Rational::parse(text).ok())
}

fn parse_rate(stream: &RawStream) -> Option<FrameRate> {
    [&stream.avg_frame_rate, &stream.r_frame_rate]
        .into_iter()
        .flatten()
        .find_map(|text| FrameRate::parse(text).ok())
}

fn convert(tool_output: RawProbe, path: &Path) -> MediaInfo {
    let mut video = None;
    let mut audio = Vec::new();
    for stream in tool_output.streams {
        match stream.codec_type.as_deref() {
            Some("video") if video.is_none() => {
                video = Some(VideoStream {
                    index: stream.index,
                    codec: stream
                        .codec_name
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    width: stream.width.unwrap_or(0),
                    height: stream.height.unwrap_or(0),
                    frame_rate: parse_rate(&stream),
                    duration: parse_duration(stream.duration.as_ref()),
                    nb_frames: stream.nb_frames.and_then(|text| text.parse().ok()),
                    color_space: stream.color_space,
                    color_primaries: stream.color_primaries,
                    color_transfer: stream.color_transfer,
                    field_order: stream.field_order,
                });
            }
            Some("audio") => {
                audio.push(AudioStream {
                    index: stream.index,
                    codec: stream
                        .codec_name
                        .clone()
                        .unwrap_or_else(|| "unknown".to_owned()),
                    sample_rate: stream
                        .sample_rate
                        .as_deref()
                        .and_then(|text| text.parse().ok())
                        .unwrap_or(0),
                    channels: stream.channels.unwrap_or(0),
                    duration: parse_duration(stream.duration.as_ref()),
                });
            }
            _ => {}
        }
    }
    MediaInfo {
        path: path.to_path_buf(),
        duration: parse_duration(tool_output.format.duration.as_ref()),
        format_name: tool_output.format.format_name,
        video,
        audio,
    }
}

/// Probes `path` with the `ffprobe` executable on `PATH`.
///
/// # Errors
///
/// Returns [`ProbeError::InputNotFound`] before spawning when the file is
/// absent, [`ProbeError::ToolMissing`] when ffprobe is unavailable, a command
/// failure for a corrupt input, or a JSON error for unexpected output.
pub fn probe_file(path: &Path) -> Result<MediaInfo, ProbeError> {
    probe_with_tool(OsStr::new("ffprobe"), path)
}

fn probe_with_tool(tool: &OsStr, path: &Path) -> Result<MediaInfo, ProbeError> {
    let tool_name = tool.to_string_lossy().into_owned();
    if !path.is_file() {
        return Err(ProbeError::InputNotFound(path.to_path_buf()));
    }
    if tool::find(&tool_name).is_none() {
        return Err(ProbeError::ToolMissing { tool: tool_name });
    }
    let output = Command::new(tool)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|source| match source.kind() {
            io::ErrorKind::NotFound => ProbeError::ToolMissing {
                tool: tool_name.clone(),
            },
            _ => ProbeError::Spawn {
                tool: tool_name.clone(),
                source,
            },
        })?;
    if !output.status.success() {
        return Err(ProbeError::CommandFailed {
            tool: tool_name,
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let raw: RawProbe = serde_json::from_slice(&output.stdout)?;
    Ok(convert(raw, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "format": {
            "filename": "sample.mov",
            "format_name": "mov,mp4,m4a,3gp,3g2,mj2",
            "duration": "10.010000"
        },
        "streams": [
            {
                "index": 0,
                "codec_type": "video",
                "codec_name": "h264",
                "width": 1920,
                "height": 1080,
                "r_frame_rate": "30000/1001",
                "avg_frame_rate": "30000/1001",
                "duration": "10.010000",
                "nb_frames": "300"
            },
            {
                "index": 1,
                "codec_type": "audio",
                "codec_name": "aac",
                "sample_rate": "48000",
                "channels": 2,
                "duration": "10.010000"
            }
        ]
    }"#;

    #[test]
    fn decodes_probe_json_with_exact_values() {
        let raw: RawProbe = serde_json::from_str(SAMPLE).unwrap();
        let info = convert(raw, Path::new("sample.mov"));
        assert_eq!(info.duration, Some(Rational::new(10_010, 1_000).unwrap()));
        assert_eq!(info.format_name.as_deref(), Some("mov,mp4,m4a,3gp,3g2,mj2"));
        let video = info.video.as_ref().unwrap();
        assert_eq!(video.frame_rate, Some(FrameRate::NTSC_2997));
        assert_eq!((video.width, video.height), (1920, 1080));
        assert_eq!(video.nb_frames, Some(300));
        let audio = info.primary_audio_stream().unwrap();
        assert_eq!((audio.sample_rate, audio.channels), (48_000, 2));
        assert_eq!(info.frame_rate_or_default(), FrameRate::NTSC_2997);
    }

    #[test]
    fn ignores_unusable_frame_rates() {
        let raw: RawProbe = serde_json::from_str(
            r#"{"format":{},"streams":[{"index":0,"codec_type":"video",
                "avg_frame_rate":"0/0","r_frame_rate":"0/0"}]}"#,
        )
        .unwrap();
        let info = convert(raw, Path::new("x"));
        assert_eq!(info.video.as_ref().unwrap().frame_rate, None);
        assert_eq!(info.frame_rate_or_default(), FrameRate::FPS_30);
        assert_eq!(info.duration_seconds(), None);
    }

    #[test]
    fn decodes_video_color_and_field_metadata() {
        let raw: RawProbe = serde_json::from_str(
            r#"{"format":{},"streams":[{"index":0,"codec_type":"video",
                "width":1920,"height":1080,"r_frame_rate":"30000/1001",
                "color_space":"bt709","color_primaries":"bt709",
                "color_transfer":"bt709","field_order":"tt"}]}"#,
        )
        .unwrap();
        let info = convert(raw, Path::new("interlaced.mov"));
        let video = info.video.as_ref().unwrap();
        assert_eq!(video.color_space.as_deref(), Some("bt709"));
        assert_eq!(video.color_primaries.as_deref(), Some("bt709"));
        assert_eq!(video.color_transfer.as_deref(), Some("bt709"));
        assert_eq!(video.field_order.as_deref(), Some("tt"));
    }

    #[test]
    fn absent_video_color_metadata_stays_none() {
        let raw: RawProbe = serde_json::from_str(
            r#"{"format":{},"streams":[{"index":0,"codec_type":"video",
                "width":320,"height":240,"color_space":null,"field_order":null}]}"#,
        )
        .unwrap();
        let video = convert(raw, Path::new("small.mp4")).video.unwrap();
        assert_eq!(video.color_space, None);
        assert_eq!(video.color_primaries, None);
        assert_eq!(video.color_transfer, None);
        assert_eq!(video.field_order, None);
        assert_eq!((video.width, video.height), (320, 240));
    }

    #[test]
    fn reports_missing_ffprobe_clearly() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("input.mp4");
        std::fs::write(&file, b"dummy").unwrap();
        let error = probe_with_tool(OsStr::new("jumpcut-ffprobe-absent-12345"), &file).unwrap_err();
        assert!(matches!(error, ProbeError::ToolMissing { .. }));
        assert!(error.to_string().contains("ffprobe"));
    }

    #[test]
    fn reports_missing_input_before_spawning() {
        let error = probe_with_tool(
            OsStr::new("ffprobe"),
            Path::new("jumpcut-no-such-file-12345.mp4"),
        )
        .unwrap_err();
        assert!(matches!(error, ProbeError::InputNotFound(_)));
    }

    #[test]
    fn probes_a_generated_wav_with_real_ffprobe() {
        if !tool::is_available("ffprobe") || !tool::is_available("ffmpeg") {
            eprintln!("skipping: ffmpeg/ffprobe not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("tone.wav");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100:duration=1",
                "-ac",
                "1",
                "-y",
            ])
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success());
        let info = probe_file(&wav).unwrap();
        assert_eq!(info.audio.len(), 1);
        assert_eq!(info.audio[0].sample_rate, 44_100);
        assert_eq!(info.audio[0].channels, 1);
        let duration = info.duration_seconds().unwrap();
        assert!((1.0 - duration).abs() < 0.05, "duration was {duration}");
    }
}

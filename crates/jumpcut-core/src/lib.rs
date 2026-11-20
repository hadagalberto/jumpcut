//! Core library for jumpcut.
//!
//! Milestone 1 provides the timing, metadata and decoding foundations used by
//! the later analysis and export stages:
//!
//! - exact [`Rational`] time and [`FrameRate`] arithmetic (no NTSC float drift);
//! - [`Timecode`] formatting/parsing, including drop-frame counting;
//! - [`probe_file`] for reading metadata through ffprobe;
//! - [`PcmDecoder`] for streaming `f32le` samples out of ffmpeg;
//! - [`analysis`] for streaming RMS/peak frames, adaptive thresholds, hysteresis,
//!   silence detection and frame-accurate edit planning (milestone 2).

#![deny(missing_docs)]
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::must_use_candidate,
    reason = "pure getters are common in this API"
)]
#![allow(
    clippy::module_name_repetitions,
    reason = "type names intentionally mirror their module"
)]

pub mod analysis;
pub mod decode;
pub mod ffprobe;
pub mod rational;
pub mod timecode;
pub mod tool;

pub use analysis::{
    Analysis, AnalysisConfig, AnalysisError, Dbfs, DetectionThresholds, Edit, FrameConfig, Margins,
    Seconds, SegmentPlan, SilenceRegion, StreamSelection, ThresholdSelection, TimeSpan,
    analyze_file, analyze_frames, analyze_samples, atempo_chain, detect_silences, parse_margins,
    resolve_thresholds, sparkline,
};
pub use decode::{DecodeError, DecodeOptions, PcmDecoder, decode_all};
pub use ffprobe::{AudioStream, MediaInfo, ProbeError, VideoStream, probe_file};
pub use rational::{FrameRate, Rational, RationalError};
pub use timecode::{Timecode, TimecodeError};

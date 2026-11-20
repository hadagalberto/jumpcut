//! Core library for jumpcut.
//!
//! The library covers timing, decoding, analysis, planning, rendering and
//! timeline export:
//!
//! - exact [`Rational`] time and [`FrameRate`] arithmetic (no NTSC float drift);
//! - [`Timecode`] formatting/parsing, including drop-frame counting;
//! - [`probe_file`] for reading metadata through ffprobe;
//! - [`PcmDecoder`] for streaming `f32le` samples out of ffmpeg;
//! - [`analysis`] for streaming RMS/peak frames, adaptive thresholds, hysteresis,
//!   silence detection and frame-accurate edit planning;
//! - [`render`] for ffmpeg rendering with automatic strategy selection, stream
//!   copy and hardware encoder passthrough;
//! - [`export`] for FCPXML, Premiere, EDL, OTIO, concat and JSON timelines;
//! - [`preview`] for the terminal summary and loudness sparkline.

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
pub mod export;
pub mod ffprobe;
pub mod preview;
pub mod rational;
pub mod render;
pub mod timecode;
pub mod tool;

pub use analysis::{
    Analysis, AnalysisConfig, AnalysisError, Dbfs, DetectionThresholds, Edit, FrameConfig, Margins,
    Seconds, SegmentPlan, SilenceRegion, StreamSelection, ThresholdSelection, TimeSpan,
    analyze_file, analyze_frames, analyze_samples, atempo_chain, detect_silences,
    frames_from_chunks, frames_from_samples, parse_margins, resolve_thresholds, sparkline,
};
pub use decode::{DecodeError, DecodeOptions, PcmDecoder, decode_all};
pub use export::{
    ExportContext, ExportError, ExportFormat, Scan, TimelineClip, VideoInfo, compact_analysis,
    export,
};
pub use ffprobe::{AudioStream, MediaInfo, ProbeError, VideoStream, probe_file};
pub use preview::{DEFAULT_SPARKLINE_WIDTH, preview, render_preview};
pub use rational::{FrameRate, Rational, RationalError};
pub use render::{
    DEFAULT_MAX_FILTER_SEGMENTS, RenderError, RenderOptions, RenderOutcome, RenderPlan,
    RenderSegment, RenderStrategy, choose_strategy, ffmpeg_args, filter_script, render,
    segment_concat_list,
};
pub use timecode::{Timecode, TimecodeError};

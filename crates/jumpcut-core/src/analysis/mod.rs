//! Silence analysis and edit planning.
//!
//! This module turns raw PCM into a [`SegmentPlan`] through four stages:
//!
//! 1. [`frame`] measures per-frame RMS/peak in dBFS with bounded memory;
//! 2. [`threshold`] resolves an absolute or adaptive (`--auto`) threshold;
//! 3. [`detect`] finds silent regions using hysteresis;
//! 4. [`plan`] applies margins, minimum durations, merging, frame snapping and
//!    silent-speed handling.
//!
//! [`file`] wires those stages to `ffprobe`/`ffmpeg` for real media files, while
//! [`analyze_samples`] and [`analyze_frames`] operate on pure-Rust PCM so the
//! whole pipeline is testable without external tools.

pub mod detect;
pub mod file;
pub mod frame;
pub mod plan;
pub mod threshold;

use std::str::FromStr;

use serde::Serialize;
use thiserror::Error;

use crate::rational::{FrameRate, Rational};

pub use detect::{SilenceRegion, detect_silences};
pub use file::{StreamSelection, analyze_file, combine_frames};
pub use frame::{
    AudioFrame, FrameAnalyzer, FrameConfig, amplitude_to_dbfs, frames_from_chunks,
    frames_from_samples,
};
pub use plan::{
    Edit, Margins, SegmentPlan, TimeSpan, atempo_chain, build_plan, parse_margins, sparkline,
};
pub use threshold::{DetectionThresholds, ThresholdSelection, auto_threshold, resolve_thresholds};

/// Floor applied to all dBFS values so they stay finite and JSON-serialisable.
pub const DBFS_FLOOR: f64 = -140.0;
/// Default absolute silence threshold in dBFS.
pub const DEFAULT_THRESHOLD_DBFS: f64 = -35.0;

/// Errors produced by the analysis pipeline.
#[derive(Debug, Error)]
pub enum AnalysisError {
    /// A decibel value could not be parsed.
    #[error("invalid decibel value `{0}`; expected e.g. -35dB or -35.5")]
    InvalidDbfs(String),
    /// A duration could not be parsed.
    #[error("invalid duration `{0}`; expected e.g. 0.5s or 500ms")]
    InvalidDuration(String),
    /// A margin specification could not be parsed.
    #[error("invalid margin `{0}`; expected a duration or `before,after`")]
    InvalidMargin(String),
    /// The supplied configuration is internally inconsistent.
    #[error("invalid analysis configuration: {0}")]
    InvalidConfig(String),
    /// Multi-stream analysis received mismatched frame grids.
    #[error("audio stream frame mismatch: expected {expected} frames, got {actual}")]
    StreamMismatch {
        /// Frames in the reference stream.
        expected: usize,
        /// Frames in the mismatching stream.
        actual: usize,
    },
    /// No audio stream was available for analysis.
    #[error("media file has no audio streams to analyse")]
    NoAudioStreams,
    /// ffprobe failed.
    #[error(transparent)]
    Probe(#[from] crate::ffprobe::ProbeError),
    /// ffmpeg decoding failed.
    #[error(transparent)]
    Decode(#[from] crate::decode::DecodeError),
}

/// A decibel value parsed from CLI text such as `-35dB` or `-35.5`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Dbfs(pub f64);

impl FromStr for Dbfs {
    type Err = AnalysisError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let trimmed = text.trim();
        let number = trimmed
            .strip_suffix("dBFS")
            .or_else(|| trimmed.strip_suffix("dB"))
            .unwrap_or(trimmed)
            .trim();
        let value = number
            .parse::<f64>()
            .map_err(|_| AnalysisError::InvalidDbfs(text.to_owned()))?;
        if !value.is_finite() {
            return Err(AnalysisError::InvalidDbfs(text.to_owned()));
        }
        Ok(Self(value))
    }
}

/// A non-negative duration parsed from `0.5`, `0.5s` or `500ms`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Seconds(pub Rational);

impl FromStr for Seconds {
    type Err = AnalysisError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let trimmed = text.trim();
        let (number, divisor) = if let Some(rest) = trimmed.strip_suffix("ms") {
            (
                rest.trim(),
                Rational::new(1_000, 1).expect("constant fraction"),
            )
        } else {
            let rest = trimmed.strip_suffix('s').unwrap_or(trimmed).trim();
            (rest, Rational::ONE)
        };
        let value = Rational::parse(number)
            .map_err(|_| AnalysisError::InvalidDuration(text.to_owned()))?
            / divisor;
        if value.is_negative() {
            return Err(AnalysisError::InvalidDuration(text.to_owned()));
        }
        Ok(Self(value))
    }
}

/// All configuration controlling analysis and planning.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AnalysisConfig {
    /// Frame measurement window and hop.
    pub frame: FrameConfig,
    /// How the base silence threshold is chosen.
    pub threshold: ThresholdSelection,
    /// Width of the hysteresis band in dB.
    pub hysteresis_db: f64,
    /// Silences shorter than this are never removed.
    pub min_silence: Rational,
    /// Kept spans shorter than this are absorbed into neighbouring cuts.
    pub min_clip: Rational,
    /// Silence retained on either side of speech.
    pub margins: Margins,
    /// Kept gaps shorter than this are merged into a single cut.
    pub merge_gap: Rational,
    /// When set, silences are sped up by this factor instead of being cut.
    pub silent_speed: Option<f64>,
    /// Frame rate to snap cut boundaries to, when known.
    pub frame_rate: Option<FrameRate>,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            frame: FrameConfig::default(),
            threshold: ThresholdSelection::Absolute(DEFAULT_THRESHOLD_DBFS),
            hysteresis_db: 3.0,
            min_silence: Rational::new(1, 2).expect("constant fraction"),
            min_clip: Rational::new(3, 10).expect("constant fraction"),
            margins: Margins::default(),
            merge_gap: Rational::ZERO,
            silent_speed: None,
            frame_rate: None,
        }
    }
}

impl AnalysisConfig {
    /// Validates every parameter before analysis starts.
    ///
    /// # Errors
    ///
    /// Returns [`AnalysisError::InvalidConfig`] when a duration is negative, a
    /// floating-point parameter is not finite, or a speed is not greater than 1.
    pub fn validate(&self) -> Result<(), AnalysisError> {
        let bad = |message: &str| AnalysisError::InvalidConfig(message.to_owned());
        if self.frame.window.0 <= Rational::ZERO || self.frame.hop.0 <= Rational::ZERO {
            return Err(bad("frame window and hop must be positive"));
        }
        if let ThresholdSelection::Absolute(db) = self.threshold {
            if !db.is_finite() {
                return Err(bad("threshold must be finite"));
            }
        }
        if !self.hysteresis_db.is_finite() || self.hysteresis_db < 0.0 {
            return Err(bad("hysteresis must be a finite, non-negative number"));
        }
        if self.min_silence.is_negative()
            || self.min_clip.is_negative()
            || self.margins.before.is_negative()
            || self.margins.after.is_negative()
            || self.merge_gap.is_negative()
        {
            return Err(bad("durations and margins must not be negative"));
        }
        if let Some(speed) = self.silent_speed {
            if !speed.is_finite() || speed <= 1.0 {
                return Err(bad("silent speed must be a finite number greater than 1"));
            }
        }
        Ok(())
    }
}

/// Complete analysis result: frames, resolved thresholds and the edit plan.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Analysis {
    /// PCM sample rate the frames were computed at.
    pub sample_rate: u32,
    /// Configuration that produced this result.
    pub config: AnalysisConfig,
    /// Resolved hysteresis thresholds.
    pub thresholds: DetectionThresholds,
    /// All analysis frames, in time order.
    pub frames: Vec<AudioFrame>,
    /// Edit plan covering the whole source.
    pub plan: SegmentPlan,
}

impl Analysis {
    /// Total duration removed by hard cuts.
    #[must_use]
    pub fn removed_duration(&self) -> Rational {
        self.plan.removed_duration()
    }

    /// Duration of the output after speed-ups.
    #[must_use]
    pub fn output_duration(&self) -> Rational {
        self.plan.output_duration()
    }
}

/// Runs detection and planning over already-computed frames.
///
/// # Errors
///
/// Returns [`AnalysisError::InvalidConfig`] for an invalid configuration.
pub fn analyze_frames(
    frames: &[AudioFrame],
    config: &AnalysisConfig,
    source_duration: Rational,
    sample_rate: u32,
) -> Result<Analysis, AnalysisError> {
    config.validate()?;
    let thresholds = resolve_thresholds(config.threshold, frames, config.hysteresis_db);
    let silences = detect_silences(frames, thresholds);
    let plan = build_plan(&silences, config, source_duration, thresholds);
    Ok(Analysis {
        sample_rate,
        config: config.clone(),
        thresholds,
        frames: frames.to_vec(),
        plan,
    })
}

/// Builds frames from PCM and runs the full analysis pipeline.
///
/// # Errors
///
/// Propagates frame-configuration and configuration errors.
pub fn analyze_samples(
    samples: &[f32],
    sample_rate: u32,
    config: &AnalysisConfig,
) -> Result<Analysis, AnalysisError> {
    let frames = frames_from_samples(samples, sample_rate, &config.frame)?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "sample counts are exact below 2^53"
    )]
    let duration = Rational::new(
        i128::try_from(samples.len()).unwrap_or(i128::MAX),
        i128::from(sample_rate),
    )
    .unwrap_or(Rational::ZERO);
    analyze_frames(&frames, config, duration, sample_rate)
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "test fixtures use small, known-safe numeric conversions"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn brisk_config() -> AnalysisConfig {
        AnalysisConfig {
            threshold: ThresholdSelection::Absolute(-35.0),
            hysteresis_db: 0.0,
            min_silence: Rational::new(1, 10).unwrap(),
            min_clip: Rational::ZERO,
            margins: Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            ..AnalysisConfig::default()
        }
    }

    /// Builds constant-amplitude PCM blocks: DC level `0.5` is about -6 dBFS,
    /// zero is digital silence and produces an exact floored reading.
    fn audio(blocks: &[(f64, f32)]) -> Vec<f32> {
        let mut samples = Vec::new();
        for &(seconds, amplitude) in blocks {
            let count = (seconds * 16_000.0) as usize;
            samples.resize(samples.len() + count, amplitude);
        }
        samples
    }

    #[test]
    fn parses_decibels_and_durations() {
        assert_eq!("-35dB".parse::<Dbfs>().unwrap(), Dbfs(-35.0));
        assert_eq!("-35.5 dBFS".parse::<Dbfs>().unwrap(), Dbfs(-35.5));
        assert_eq!("-35".parse::<Dbfs>().unwrap(), Dbfs(-35.0));
        assert!("abc".parse::<Dbfs>().is_err());
        assert_eq!(
            "0.5s".parse::<Seconds>().unwrap(),
            Seconds(Rational::new(1, 2).unwrap())
        );
        assert_eq!(
            "500ms".parse::<Seconds>().unwrap(),
            Seconds(Rational::new(1, 2).unwrap())
        );
        assert_eq!(
            "2".parse::<Seconds>().unwrap(),
            Seconds(Rational::new(2, 1).unwrap())
        );
        assert!("-1s".parse::<Seconds>().is_err());
    }

    #[test]
    fn config_validation_rejects_bad_values() {
        let mut config = brisk_config();
        assert!(config.validate().is_ok());
        config.hysteresis_db = -1.0;
        assert!(config.validate().is_err());
        config.hysteresis_db = 1.0;
        config.silent_speed = Some(1.0);
        assert!(config.validate().is_err());
        config.silent_speed = Some(4.0);
        config.margins.before = Rational::new(-1, 10).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn synthetic_pcm_produces_exact_segments() {
        let samples = audio(&[(1.0, 0.5), (1.0, 0.0), (1.0, 0.5), (0.5, 0.0), (0.5, 0.5)]);
        let analysis = analyze_samples(&samples, 16_000, &brisk_config()).unwrap();
        assert_eq!(analysis.frames.len(), 400);

        let kept: Vec<(Rational, Rational)> = analysis
            .plan
            .kept_segments()
            .into_iter()
            .map(|span| (span.start, span.end))
            .collect();
        let cuts: Vec<(Rational, Rational)> = analysis
            .plan
            .cut_segments()
            .into_iter()
            .map(|span| (span.start, span.end))
            .collect();
        let sec = |n: i128, d: i128| Rational::new(n, d).unwrap();

        assert_eq!(
            kept,
            vec![
                (sec(0, 1), sec(1, 1)),
                (sec(2, 1), sec(3, 1)),
                (sec(7, 2), sec(4, 1))
            ]
        );
        assert_eq!(cuts, vec![(sec(1, 1), sec(2, 1)), (sec(3, 1), sec(7, 2))]);
        assert_eq!(analysis.plan.output_duration(), sec(5, 2));
        assert_eq!(analysis.removed_duration(), sec(3, 2));
    }

    #[test]
    fn margins_keep_silence_adjacent_to_speech() {
        let samples = audio(&[(1.0, 0.5), (1.0, 0.0), (1.0, 0.5)]);
        let mut config = brisk_config();
        config.margins = Margins {
            before: Rational::new(1, 10).unwrap(),
            after: Rational::new(2, 10).unwrap(),
        };
        let analysis = analyze_samples(&samples, 16_000, &config).unwrap();
        let cuts = analysis.plan.cut_segments();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].start, Rational::new(11, 10).unwrap());
        assert_eq!(cuts[0].end, Rational::new(18, 10).unwrap());
    }

    #[test]
    fn auto_mode_detects_silence_between_tone_and_noise() {
        let mut samples = audio(&[(1.0, 0.4), (2.0, 0.0), (1.0, 0.4)]);
        // Add low-level background noise to the "silent" part so the 10th and
        // 90th percentiles have a meaningful spread.
        for (index, sample) in samples.iter_mut().enumerate() {
            if (16_000..48_000).contains(&index) && index % 3 == 0 {
                *sample = 0.001;
            }
        }
        let config = AnalysisConfig {
            threshold: ThresholdSelection::Auto,
            hysteresis_db: 0.0,
            min_silence: Rational::new(1, 2).unwrap(),
            min_clip: Rational::ZERO,
            margins: Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            ..AnalysisConfig::default()
        };
        let analysis = analyze_samples(&samples, 16_000, &config).unwrap();
        assert!(analysis.thresholds.adaptive);
        let cuts = analysis.plan.cut_segments();
        assert_eq!(cuts.len(), 1, "cuts were {cuts:?}");
        assert!(cuts[0].start <= Rational::new(11, 10).unwrap());
        assert!(cuts[0].end >= Rational::new(19, 10).unwrap());
    }

    fn rational_between(min: i128, max: i128) -> impl Strategy<Value = Rational> {
        (min..=max).prop_map(|num| Rational::new(num, 100).unwrap())
    }

    proptest! {
        #[test]
        fn plan_invariants_hold(
            levels in prop::collection::vec(
                prop_oneof![Just(-140.0), Just(-60.0), Just(-36.0), Just(-6.0)],
                1..80
            ),
            min_silence in rational_between(0, 30),
            min_clip in rational_between(0, 30),
            margin in rational_between(0, 20),
            merge_gap in rational_between(0, 30),
            snap in any::<bool>(),
            speed in prop_oneof![Just(0u8), Just(1)],
        ) {
            let config = AnalysisConfig {
                threshold: ThresholdSelection::Absolute(-35.0),
                hysteresis_db: 0.0,
                min_silence,
                min_clip,
                margins: Margins { before: margin, after: margin },
                merge_gap,
                silent_speed: if speed == 1 { Some(4.0) } else { None },
                frame_rate: if snap { Some(FrameRate::FPS_30) } else { None },
                ..AnalysisConfig::default()
            };
            let frames = frames_from_levels(&levels);
            let duration = frames.last().map_or(Rational::ZERO, |frame| frame.start + frame.window_duration);
            let analysis = analyze_frames(&frames, &config, duration, 16_000).unwrap();
            let plan = &analysis.plan;

            // Edits are sorted, non-overlapping and cover [0, duration).
            let mut cursor = Rational::ZERO;
            for edit in &plan.edits {
                let span = edit.span();
                prop_assert!(span.start == cursor, "gap or overlap at {:?}", span);
                prop_assert!(span.end >= span.start);
                cursor = span.end;
            }
            prop_assert_eq!(cursor, duration);

            let total = plan.edits.iter().fold(Rational::ZERO, |sum, edit| sum + edit.span().duration());
            prop_assert_eq!(total, duration);

            // A cut never appears as a retained segment and vice versa.
            let kept = plan.kept_segments();
            let cuts = plan.cut_segments();
            prop_assert!(cuts.iter().all(|span| !kept.contains(span)));
            let speech: Vec<TimeSpan> = plan.edits.iter().filter_map(|edit| match edit {
                Edit::Keep { span } => Some(*span),
                _ => None,
            }).collect();
            if speech.len() > 1 {
                prop_assert!(
                    speech.iter().all(|span| span.duration() >= min_clip),
                    "speech={speech:?} min_clip={min_clip:?} edits={:?}",
                    plan.edits
                );
            }
            prop_assert!(plan.output_duration() <= duration);
            if plan.edits.iter().any(|edit| matches!(edit, Edit::SpeedUp { .. })) {
                prop_assert_eq!(plan.removed_duration(), Rational::ZERO);
                prop_assert!(plan.output_duration() < duration);
            } else {
                prop_assert_eq!(plan.removed_duration() + plan.output_duration(), duration);
            }

            if snap {
                for edit in &plan.edits {
                    let span = edit.span();
                    for boundary in [span.start, span.end] {
                        if boundary > Rational::ZERO && boundary < duration {
                            let rate = FrameRate::FPS_30;
                            prop_assert_eq!(boundary, rate.duration_of_frames(boundary.frame_count(rate)));
                        }
                    }
                }
            }
        }
    }

    fn frames_from_levels(levels: &[f64]) -> Vec<AudioFrame> {
        let frames: Vec<AudioFrame> = levels
            .iter()
            .enumerate()
            .map(|(index, &level)| AudioFrame {
                index: index as u64,
                start: Rational::new(index as i128, 100).unwrap(),
                duration: Rational::new(1, 100).unwrap(),
                window_duration: Rational::new(3, 100).unwrap(),
                rms_dbfs: level,
                peak_dbfs: level,
            })
            .collect();
        frames
    }

    struct TwoHours {
        chunk_index: u64,
        chunk_samples: usize,
        chunks: u64,
    }

    impl Iterator for TwoHours {
        type Item = Vec<f32>;

        fn next(&mut self) -> Option<Self::Item> {
            if self.chunk_index >= self.chunks {
                return None;
            }
            // 8000 samples = 0.5 s; four chunks speech, four chunks silence.
            let speech = (self.chunk_index % 8) < 4;
            let value = if speech { 0.25 } else { 0.0 };
            self.chunk_index += 1;
            Some(vec![value; self.chunk_samples])
        }
    }

    #[test]
    fn two_hour_performance_bound() {
        use std::time::{Duration, Instant};
        let config = brisk_config();
        let source = TwoHours {
            chunk_index: 0,
            chunk_samples: 8_000,
            chunks: 14_400,
        };
        let started = Instant::now();
        let frames = frames_from_chunks(source, 16_000, &config.frame).unwrap();
        let analysis =
            analyze_frames(&frames, &config, Rational::new(7_200, 1).unwrap(), 16_000).unwrap();
        let elapsed = started.elapsed();
        assert_eq!(frames.len(), 720_000);
        assert_eq!(analysis.plan.cut_segments().len(), 1_800);
        let bound = if cfg!(debug_assertions) {
            Duration::from_secs(180)
        } else {
            Duration::from_secs(3)
        };
        assert!(
            elapsed < bound,
            "2 h analysis took {elapsed:?} (bound {bound:?})"
        );
    }
}

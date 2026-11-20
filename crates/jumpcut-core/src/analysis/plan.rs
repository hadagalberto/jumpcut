//! Turning silence regions into a frame-accurate edit plan.
//!
//! The planner applies, in order: minimum-silence filtering, per-side margins,
//! short-gap merging, frame snapping, and minimum-clip enforcement. The result
//! is a contiguous list of [`Edit`]s spanning the whole source, so consumers
//! (preview, JSON and later renderers/exporters) share one representation.

use serde::Serialize;

use super::AnalysisConfig;
use super::detect::SilenceRegion;
use super::frame::AudioFrame;
use super::threshold::DetectionThresholds;
use crate::rational::{FrameRate, Rational};

/// Silence kept around each side of speech, in seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Margins {
    /// Silence retained immediately *after* preceding speech (before a cut).
    pub before: Rational,
    /// Silence retained immediately *before* following speech (after a cut).
    pub after: Rational,
}

impl Default for Margins {
    fn default() -> Self {
        Self {
            before: Rational::new(1, 10).expect("constant fraction"),
            after: Rational::new(1, 10).expect("constant fraction"),
        }
    }
}

/// Parses `0.1s` (both sides) or `0.08s,0.15s` (before, after) into margins.
///
/// # Errors
///
/// Returns [`AnalysisError::InvalidMargin`](super::AnalysisError::InvalidMargin)
/// when either side is not a non-negative duration.
pub fn parse_margins(text: &str) -> Result<Margins, super::AnalysisError> {
    use super::Seconds;
    use std::str::FromStr;
    let trimmed = text.trim();
    if let Some((left, right)) = trimmed.split_once(',') {
        let before = Seconds::from_str(left)
            .map_err(|_| super::AnalysisError::InvalidMargin(text.to_owned()))?;
        let after = Seconds::from_str(right)
            .map_err(|_| super::AnalysisError::InvalidMargin(text.to_owned()))?;
        Ok(Margins {
            before: before.0,
            after: after.0,
        })
    } else {
        let value = Seconds::from_str(trimmed)
            .map_err(|_| super::AnalysisError::InvalidMargin(text.to_owned()))?;
        Ok(Margins {
            before: value.0,
            after: value.0,
        })
    }
}

/// An exact time interval `[start, end)` in seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TimeSpan {
    /// Inclusive start time.
    pub start: Rational,
    /// Exclusive end time.
    pub end: Rational,
}

impl TimeSpan {
    /// Length of the span.
    #[must_use]
    pub fn duration(&self) -> Rational {
        self.end - self.start
    }
}

/// One contiguous edit covering part of the source timeline.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Edit {
    /// Play this span at normal speed.
    Keep {
        /// Source time interval retained in the output.
        span: TimeSpan,
    },
    /// Retain this span but play it back faster instead of cutting it.
    SpeedUp {
        /// Source time interval retained in the output.
        span: TimeSpan,
        /// Playback speed multiplier (strictly greater than 1).
        speed: f64,
    },
    /// Remove this span from the output.
    Cut {
        /// Source time interval removed from the output.
        span: TimeSpan,
    },
}

impl Edit {
    /// The source time span covered by this edit.
    #[must_use]
    pub fn span(&self) -> TimeSpan {
        match self {
            Self::Keep { span } | Self::SpeedUp { span, .. } | Self::Cut { span } => *span,
        }
    }

    /// Whether this edit retains source audio in the output.
    #[must_use]
    pub const fn is_kept(&self) -> bool {
        !matches!(self, Self::Cut { .. })
    }
}

/// A complete silence-removal plan over the whole source.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SegmentPlan {
    /// Total source duration in seconds.
    pub source_duration: Rational,
    /// Frame rate boundaries were snapped to, when one was available.
    pub frame_rate: Option<FrameRate>,
    /// Resolved detection thresholds.
    pub thresholds: DetectionThresholds,
    /// Raw detected silence regions, before margins and cleanup.
    pub silence_regions: Vec<SilenceRegion>,
    /// Contiguous edits covering `[0, source_duration)`.
    pub edits: Vec<Edit>,
}

impl SegmentPlan {
    /// Spans retained in the output (normal speed or sped up).
    #[must_use]
    pub fn kept_segments(&self) -> Vec<TimeSpan> {
        self.edits
            .iter()
            .filter(|edit| edit.is_kept())
            .map(Edit::span)
            .collect()
    }

    /// Spans cut out of the output.
    #[must_use]
    pub fn cut_segments(&self) -> Vec<TimeSpan> {
        self.edits
            .iter()
            .filter_map(|edit| match edit {
                Edit::Cut { span } => Some(*span),
                _ => None,
            })
            .collect()
    }

    /// Duration of the rendered output after speed-ups are applied.
    #[must_use]
    pub fn output_duration(&self) -> Rational {
        self.edits.iter().fold(Rational::ZERO, |total, edit| {
            let span = edit.span().duration();
            match edit {
                Edit::Cut { .. } => total,
                Edit::Keep { .. } => total + span,
                Edit::SpeedUp { speed, .. } => total + span / speed_ratio(*speed),
            }
        })
    }

    /// Total duration removed by hard cuts.
    #[must_use]
    pub fn removed_duration(&self) -> Rational {
        self.cut_segments()
            .iter()
            .fold(Rational::ZERO, |total, span| total + span.duration())
    }

    /// Number of edits in the plan.
    #[must_use]
    pub fn edit_count(&self) -> usize {
        self.edits.len()
    }
}

fn speed_ratio(speed: f64) -> Rational {
    Rational::parse(&format!("{speed}")).unwrap_or(Rational::ONE)
}

type Span = (Rational, Rational);

fn normalize_cuts(cuts: Vec<Span>, duration: Rational) -> Vec<Span> {
    let mut sorted = cuts;
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut result: Vec<Span> = Vec::with_capacity(sorted.len());
    for (start, end) in sorted {
        let start = start.max(Rational::ZERO).min(duration);
        let end = end.max(Rational::ZERO).min(duration);
        if start >= end {
            continue;
        }
        if let Some(last) = result.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        result.push((start, end));
    }
    result
}

fn merge_close_cuts(cuts: Vec<Span>, gap: Rational, duration: Rational) -> Vec<Span> {
    let normalized = normalize_cuts(cuts, duration);
    let mut result: Vec<Span> = Vec::with_capacity(normalized.len());
    for (start, end) in normalized {
        if let Some(last) = result.last_mut() {
            if start <= last.1 + gap {
                last.1 = last.1.max(end);
                continue;
            }
        }
        result.push((start, end));
    }
    result
}

fn gap_at(index: usize, cuts: &[Span], duration: Rational) -> Span {
    if cuts.is_empty() {
        return (Rational::ZERO, duration);
    }
    match index {
        0 => (Rational::ZERO, cuts[0].0),
        i if i == cuts.len() => (cuts[i - 1].1, duration),
        i => (cuts[i - 1].1, cuts[i].0),
    }
}

/// Removes kept spans shorter than `min_clip` by absorbing them into the
/// neighbouring cut. A single kept span covering the whole source is exempt:
/// an entirely speech file must never be reduced to an empty plan.
fn enforce_min_clip(cuts: Vec<Span>, duration: Rational, min_clip: Rational) -> Vec<Span> {
    if cuts.is_empty() || min_clip.is_zero() {
        return normalize_cuts(cuts, duration);
    }
    let mut cuts = cuts;
    loop {
        let count = cuts.len();
        let mut changed = false;
        for index in 0..=count {
            let (start, end) = gap_at(index, &cuts, duration);
            let length = end - start;
            if length <= Rational::ZERO || length >= min_clip {
                continue;
            }
            match index {
                0 => cuts[0].0 = Rational::ZERO,
                i if i == count => cuts[count - 1].1 = duration,
                i => {
                    cuts[i - 1].1 = cuts[i].1;
                    cuts.remove(i);
                }
            }
            changed = true;
            break;
        }
        if !changed {
            break;
        }
    }
    normalize_cuts(cuts, duration)
}

fn build_edits(cuts: Vec<Span>, duration: Rational, silent_speed: Option<f64>) -> Vec<Edit> {
    let mut edits = Vec::with_capacity(cuts.len() * 2 + 1);
    let mut cursor = Rational::ZERO;
    for (start, end) in cuts {
        if start > cursor {
            edits.push(Edit::Keep {
                span: TimeSpan {
                    start: cursor,
                    end: start,
                },
            });
        }
        let span = TimeSpan { start, end };
        match silent_speed {
            Some(speed) => edits.push(Edit::SpeedUp { span, speed }),
            None => edits.push(Edit::Cut { span }),
        }
        cursor = end;
    }
    if cursor < duration {
        edits.push(Edit::Keep {
            span: TimeSpan {
                start: cursor,
                end: duration,
            },
        });
    }
    edits
}

/// Builds the complete plan from raw silence regions.
#[must_use]
pub fn build_plan(
    silences: &[SilenceRegion],
    config: &AnalysisConfig,
    source_duration: Rational,
    thresholds: DetectionThresholds,
) -> SegmentPlan {
    let mut cuts: Vec<Span> = Vec::new();
    for region in silences {
        if region.duration() < config.min_silence {
            continue;
        }
        let start = region.start + config.margins.before;
        let end = region.end - config.margins.after;
        if start < end {
            cuts.push((start, end));
        }
    }

    let mut cuts = merge_close_cuts(cuts, config.merge_gap, source_duration);
    if let Some(rate) = config.frame_rate {
        for (start, end) in &mut cuts {
            *start = start.snapped_to_frame(rate);
            *end = end.snapped_to_frame(rate);
        }
    }
    let cuts = enforce_min_clip(cuts, source_duration, config.min_clip);
    let edits = build_edits(cuts, source_duration, config.silent_speed);

    SegmentPlan {
        source_duration,
        frame_rate: config.frame_rate,
        thresholds,
        silence_regions: silences.to_vec(),
        edits,
    }
}

/// Splits a speed-up factor into chained `atempo` factors ffmpeg accepts.
///
/// ffmpeg's `atempo` filter only accepts 0.5..=4.0. Larger speeds are split
/// into several chained filters whose product is the requested speed, e.g.
/// `16 -> [4.0, 4.0]` and `6 -> [4.0, 1.5]`.
#[must_use]
pub fn atempo_chain(speed: f64) -> Vec<f64> {
    if !speed.is_finite() || speed <= 1.0 {
        return vec![1.0];
    }
    let mut factors = Vec::new();
    let mut remaining = speed;
    while remaining > 4.0 + f64::EPSILON {
        factors.push(4.0);
        remaining /= 4.0;
    }
    if remaining > 1.0 {
        factors.push(remaining);
    }
    if factors.is_empty() {
        factors.push(speed);
    }
    factors
}

const SPARKLINE_RAMP: &[u8] = b" .:-=+*#%@";

/// Renders frame loudness as an ASCII sparkline of the requested width.
///
/// RMS values from -60 dBFS to -3 dBFS map across the ramp; quieter frames
/// render as a space and louder frames as `@`.
#[must_use]
pub fn sparkline(frames: &[AudioFrame], width: usize) -> String {
    if width == 0 || frames.is_empty() {
        return String::new();
    }
    let mut output = String::with_capacity(width);
    for bucket in 0..width {
        let from = bucket * frames.len() / width;
        let to = ((bucket + 1) * frames.len() / width)
            .max(from + 1)
            .min(frames.len());
        #[allow(clippy::cast_precision_loss, reason = "bucket lengths are small")]
        let count = (to - from) as f64;
        let average = frames[from..to]
            .iter()
            .map(|frame| frame.rms_dbfs)
            .sum::<f64>()
            / count;
        let normalized = ((average + 60.0) / 57.0).clamp(0.0, 1.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "ramp index is bounded by the ramp length"
        )]
        let index = (normalized * (SPARKLINE_RAMP.len() - 1) as f64).round() as usize;
        output.push(char::from(SPARKLINE_RAMP[index]));
    }
    output
}

#[cfg(test)]
#[allow(
    clippy::assert_is_empty,
    clippy::clone_on_copy,
    reason = "test fixtures are intentionally explicit"
)]
mod tests {
    use super::*;
    use crate::analysis::Seconds;
    use crate::analysis::threshold::ThresholdSelection;

    fn config() -> AnalysisConfig {
        AnalysisConfig {
            frame: crate::analysis::frame::FrameConfig {
                window: Seconds(Rational::new(3, 100).unwrap()),
                hop: Seconds(Rational::new(1, 100).unwrap()),
            },
            threshold: ThresholdSelection::Absolute(-35.0),
            hysteresis_db: 0.0,
            min_silence: Rational::ZERO,
            min_clip: Rational::ZERO,
            margins: Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            merge_gap: Rational::ZERO,
            silent_speed: None,
            frame_rate: None,
        }
    }

    fn region(start: i128, end: i128) -> SilenceRegion {
        SilenceRegion {
            first_frame: 0,
            end_frame: 1,
            start: Rational::new(start, 1).unwrap(),
            end: Rational::new(end, 1).unwrap(),
        }
    }

    fn thresholds() -> DetectionThresholds {
        DetectionThresholds {
            silence_dbfs: -35.0,
            speech_dbfs: -35.0,
            base_dbfs: -35.0,
            adaptive: false,
        }
    }

    #[test]
    fn margins_shrink_cuts_on_each_side() {
        let mut cfg = config();
        cfg.margins = Margins {
            before: Rational::new(1, 10).unwrap(),
            after: Rational::new(2, 10).unwrap(),
        };
        let plan = build_plan(
            &[region(2, 5)],
            &cfg,
            Rational::new(10, 1).unwrap(),
            thresholds(),
        );
        let cuts = plan.cut_segments();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].start, Rational::new(21, 10).unwrap());
        assert_eq!(cuts[0].end, Rational::new(48, 10).unwrap());
    }

    #[test]
    fn min_silence_discards_short_regions() {
        let mut cfg = config();
        cfg.min_silence = Rational::new(3, 2).unwrap();
        let plan = build_plan(
            &[region(1, 2), region(4, 8)],
            &cfg,
            Rational::new(10, 1).unwrap(),
            thresholds(),
        );
        let cuts = plan.cut_segments();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].start, Rational::new(4, 1).unwrap());
    }

    #[test]
    fn merge_gap_joins_close_cuts() {
        let mut cfg = config();
        cfg.merge_gap = Rational::new(1, 1).unwrap();
        let plan = build_plan(
            &[region(1, 2), region(3, 4)],
            &cfg,
            Rational::new(10, 1).unwrap(),
            thresholds(),
        );
        let cuts = plan.cut_segments();
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0].start, Rational::new(1, 1).unwrap());
        assert_eq!(cuts[0].end, Rational::new(4, 1).unwrap());
    }

    #[test]
    fn min_clip_absorbs_short_kept_span() {
        let mut cfg = config();
        cfg.min_clip = Rational::new(2, 1).unwrap();
        let plan = build_plan(
            &[region(1, 2), region(3, 4)],
            &cfg,
            Rational::new(10, 1).unwrap(),
            thresholds(),
        );
        let cuts = plan.cut_segments();
        assert_eq!(cuts.len(), 1);
        // The short leading kept span (0..1) is absorbed too.
        assert_eq!(cuts[0].start, Rational::ZERO);
        assert_eq!(cuts[0].end, Rational::new(4, 1).unwrap());
    }

    #[test]
    fn frame_snapping_lands_on_exact_frame_boundaries() {
        let mut cfg = config();
        cfg.frame_rate = Some(FrameRate::NTSC_2997);
        let plan = build_plan(
            &[region(1, 3)],
            &cfg,
            Rational::new(10, 1).unwrap(),
            thresholds(),
        );
        let rate = FrameRate::NTSC_2997;
        for cut in plan.cut_segments() {
            for boundary in [cut.start, cut.end] {
                let frames = boundary.frame_count(rate);
                assert_eq!(boundary, rate.duration_of_frames(frames));
            }
        }
    }

    #[test]
    fn silent_speed_replaces_cuts_and_shortens_output() {
        let mut cfg = config();
        cfg.silent_speed = Some(4.0);
        let plan = build_plan(
            &[region(2, 6)],
            &cfg,
            Rational::new(10, 1).unwrap(),
            thresholds(),
        );
        assert!(plan.cut_segments().is_empty());
        // 10 s source, 4 s silence sped up 4x -> 6 s + 1 s = 7 s.
        assert_eq!(plan.output_duration(), Rational::new(7, 1).unwrap());
        match &plan.edits[1] {
            Edit::SpeedUp { speed, .. } => assert_eq!(*speed, 4.0),
            other => panic!("expected speed-up, got {other:?}"),
        }
    }

    #[test]
    fn atempo_chaining_covers_large_factors() {
        assert_eq!(atempo_chain(4.0), vec![4.0]);
        assert_eq!(atempo_chain(16.0), vec![4.0, 4.0]);
        let chain = atempo_chain(6.0);
        assert_eq!(chain.len(), 2);
        let product: f64 = chain.iter().product();
        assert!((product - 6.0).abs() < 1e-9);
    }

    #[test]
    fn sparkline_maps_quiet_and_loud_frames() {
        let loud = AudioFrame {
            index: 0,
            start: Rational::ZERO,
            duration: Rational::new(1, 100).unwrap(),
            window_duration: Rational::new(3, 100).unwrap(),
            rms_dbfs: -3.0,
            peak_dbfs: -3.0,
        };
        let quiet = AudioFrame {
            rms_dbfs: -100.0,
            peak_dbfs: -100.0,
            ..loud.clone()
        };
        assert_eq!(sparkline(&[quiet; 4], 4), "    ");
        assert_eq!(sparkline(&[loud; 4], 4), "@@@@");
        assert!(sparkline(&[], 4).is_empty());
    }

    #[test]
    fn parses_both_margin_forms() {
        assert_eq!(
            parse_margins("0.1s").unwrap(),
            Margins {
                before: Rational::new(1, 10).unwrap(),
                after: Rational::new(1, 10).unwrap()
            }
        );
        assert_eq!(
            parse_margins("80ms,150ms").unwrap(),
            Margins {
                before: Rational::new(8, 100).unwrap(),
                after: Rational::new(15, 100).unwrap()
            }
        );
        assert!(parse_margins("abc").is_err());
    }
}

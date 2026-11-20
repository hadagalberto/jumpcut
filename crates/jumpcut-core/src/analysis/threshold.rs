//! Threshold resolution, including the documented adaptive (`--auto`) mode.
//!
//! # Adaptive threshold formula
//!
//! Let `l_i = clamp(frame_i.rms_dbfs, FLOOR, 0)` be the RMS loudness of every
//! analysis frame, and let `P(p)` be the linearly interpolated `p`-quantile of
//! the sorted values:
//!
//! ```text
//! noise  = P(0.10)                      // 10th percentile: quietest frames
//! speech = P(0.90)                      // 90th percentile: loudest frames
//! base   = noise + MIX * (speech-noise) // midpoint on the dB scale, MIX = 0.5
//! ```
//!
//! If `speech - noise < MIN_RANGE_DB` the recording has no usable dynamic
//! range (e.g. constant tone or constant silence) and the fixed absolute
//! default of -35 dBFS is used instead. Hysteresis then derives the two
//! decision levels by placing `base` halfway between them:
//!
//! ```text
//! silence_dbfs = base - hysteresis/2   // level must fall below this to enter silence
//! speech_dbfs  = base + hysteresis/2   // level must rise above this to leave silence
//! ```

use super::frame::AudioFrame;
use super::{DBFS_FLOOR, DEFAULT_THRESHOLD_DBFS};

/// Fraction of the noise-to-speech dB interval used by `--auto`.
pub const AUTO_MIX: f64 = 0.5;
/// Minimum dynamic range for which the adaptive estimate is trusted.
pub const AUTO_MIN_RANGE_DB: f64 = 3.0;

/// Selects the base silence threshold.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub enum ThresholdSelection {
    /// Fixed absolute threshold in dBFS (e.g. `-35`).
    Absolute(f64),
    /// Estimate the threshold from the frame loudness distribution.
    Auto,
}

/// The two levels used by the hysteresis state machine.
///
/// A frame is considered silent when its RMS drops strictly below
/// [`Self::silence_dbfs`]; a silent region ends only when RMS rises strictly
/// above [`Self::speech_dbfs`]. Since `speech_dbfs >= silence_dbfs`, a noisy
/// signal hovering around one level cannot make the detector flap.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct DetectionThresholds {
    /// Enter silence when RMS falls below this level (dBFS).
    pub silence_dbfs: f64,
    /// Leave silence when RMS rises above this level (dBFS).
    pub speech_dbfs: f64,
    /// Base threshold before hysteresis was applied (dBFS).
    pub base_dbfs: f64,
    /// Whether the base was derived adaptively.
    pub adaptive: bool,
}

/// Linearly interpolated quantile of an already-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return DBFS_FLOOR;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "quantile position is a small index"
    )]
    let position = p * (sorted.len() - 1) as f64;
    let low = position.floor();
    let high = position.ceil();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "quantile indices are within the slice"
    )]
    let low_index = low as usize;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "quantile indices are within the slice"
    )]
    let high_index = high as usize;
    let weight = position - low;
    sorted[low_index] + (sorted[high_index] - sorted[low_index]) * weight
}

/// Computes the adaptive base threshold from frame loudness.
///
/// See the [module documentation](self) for the exact formula. The result is
/// always finite, at or below 0 dBFS and at or above [`DBFS_FLOOR`].
#[must_use]
pub fn auto_threshold(frames: &[AudioFrame]) -> f64 {
    if frames.is_empty() {
        return DEFAULT_THRESHOLD_DBFS;
    }
    let mut values: Vec<f64> = frames
        .iter()
        .map(|frame| frame.rms_dbfs.clamp(DBFS_FLOOR, 0.0))
        .collect();
    values.sort_by(f64::total_cmp);
    let noise = percentile(&values, 0.10);
    let speech = percentile(&values, 0.90);
    if speech - noise < AUTO_MIN_RANGE_DB {
        return DEFAULT_THRESHOLD_DBFS;
    }
    (noise + AUTO_MIX * (speech - noise)).clamp(DBFS_FLOOR, 0.0)
}

/// Resolves a threshold selection into the two hysteresis decision levels.
#[must_use]
pub fn resolve_thresholds(
    selection: ThresholdSelection,
    frames: &[AudioFrame],
    hysteresis_db: f64,
) -> DetectionThresholds {
    let (base, adaptive) = match selection {
        ThresholdSelection::Absolute(db) => (db, false),
        ThresholdSelection::Auto => (auto_threshold(frames), true),
    };
    let half = (hysteresis_db.max(0.0) / 2.0).min(30.0);
    DetectionThresholds {
        silence_dbfs: (base - half).clamp(DBFS_FLOOR, 0.0),
        speech_dbfs: (base + half).clamp(DBFS_FLOOR, 0.0),
        base_dbfs: base,
        adaptive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rational::Rational;

    fn frame(rms_dbfs: f64) -> AudioFrame {
        AudioFrame {
            index: 0,
            start: Rational::ZERO,
            duration: Rational::new(1, 100).unwrap(),
            window_duration: Rational::new(3, 100).unwrap(),
            rms_dbfs,
            peak_dbfs: rms_dbfs,
        }
    }

    #[test]
    fn percentile_interpolates() {
        let values = [0.0, 10.0, 20.0, 30.0];
        assert_eq!(percentile(&values, 0.0), 0.0);
        assert_eq!(percentile(&values, 1.0), 30.0);
        assert!((percentile(&values, 0.5) - 15.0).abs() < 1e-9);
        assert!((percentile(&values, 0.25) - 7.5).abs() < 1e-9);
        assert_eq!(percentile(&[], 0.5), DBFS_FLOOR);
    }

    #[test]
    fn auto_threshold_is_midpoint_of_percentiles() {
        // 80% noise at -60 and 20% speech at -10: P10 = -60, P90 = -10.
        let mut frames = vec![frame(-60.0); 80];
        frames.extend(std::iter::repeat_with(|| frame(-10.0)).take(20));
        let base = auto_threshold(&frames);
        assert!((base - (-35.0)).abs() < 1e-6, "base was {base}");
    }

    #[test]
    fn auto_threshold_falls_back_without_dynamic_range() {
        assert_eq!(
            auto_threshold(&[frame(-20.0), frame(-20.0)]),
            DEFAULT_THRESHOLD_DBFS
        );
        assert_eq!(auto_threshold(&[]), DEFAULT_THRESHOLD_DBFS);
        assert_eq!(
            auto_threshold(&[frame(DBFS_FLOOR); 10]),
            DEFAULT_THRESHOLD_DBFS
        );
    }

    #[test]
    fn hysteresis_widens_the_decision_band() {
        let frames = [frame(DBFS_FLOOR), frame(-6.0)];
        let thresholds = resolve_thresholds(ThresholdSelection::Auto, &frames, 4.0);
        assert!(thresholds.adaptive);
        assert!((thresholds.speech_dbfs - thresholds.silence_dbfs - 4.0).abs() < 1e-9);
        assert!(thresholds.silence_dbfs < thresholds.base_dbfs);
        assert!(thresholds.speech_dbfs > thresholds.base_dbfs);

        let fixed = resolve_thresholds(ThresholdSelection::Absolute(-35.0), &frames, 0.0);
        assert!(!fixed.adaptive);
        assert_eq!(fixed.silence_dbfs, -35.0);
        assert_eq!(fixed.speech_dbfs, -35.0);
    }

    #[test]
    fn absolute_threshold_is_used_verbatim() {
        let thresholds = resolve_thresholds(ThresholdSelection::Absolute(-42.0), &[], 2.0);
        assert_eq!(thresholds.base_dbfs, -42.0);
        assert_eq!(thresholds.silence_dbfs, -43.0);
        assert_eq!(thresholds.speech_dbfs, -41.0);
    }
}

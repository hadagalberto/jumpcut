//! Hysteresis silence detection over analysis frames.

use serde::Serialize;

use super::frame::AudioFrame;
use super::threshold::DetectionThresholds;
use crate::rational::Rational;

/// A contiguous run of frames classified as silent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SilenceRegion {
    /// Index of the first silent frame (inclusive).
    pub first_frame: usize,
    /// Index just past the last silent frame (exclusive).
    pub end_frame: usize,
    /// Start time in seconds (start of `first_frame`'s measurement window).
    pub start: Rational,
    /// End time in seconds (end of the last frame's measurement window).
    pub end: Rational,
}

impl SilenceRegion {
    /// Duration of the region in seconds.
    #[must_use]
    pub fn duration(&self) -> Rational {
        self.end - self.start
    }
}

fn region_span(frames: &[AudioFrame], first: usize, end: usize) -> SilenceRegion {
    let last = &frames[end - 1];
    SilenceRegion {
        first_frame: first,
        end_frame: end,
        start: frames[first].start,
        end: last.start + last.window_duration,
    }
}

/// Finds silent regions using a two-level hysteresis state machine.
///
/// Starting in the speech state, a region opens on the first frame whose RMS
/// falls below [`DetectionThresholds::silence_dbfs`] and closes only when a
/// frame rises above [`DetectionThresholds::speech_dbfs`]. A region still open
/// at the end of the input is closed at the final frame's window end.
#[must_use]
pub fn detect_silences(
    frames: &[AudioFrame],
    thresholds: DetectionThresholds,
) -> Vec<SilenceRegion> {
    let mut regions = Vec::new();
    let mut in_silence = false;
    let mut first = 0_usize;

    for (index, frame) in frames.iter().enumerate() {
        if !in_silence {
            if frame.rms_dbfs < thresholds.silence_dbfs {
                in_silence = true;
                first = index;
            }
        } else if frame.rms_dbfs > thresholds.speech_dbfs {
            regions.push(region_span(frames, first, index));
            in_silence = false;
        }
    }
    if in_silence {
        regions.push(region_span(frames, first, frames.len()));
    }
    regions
}

#[cfg(test)]
#[allow(
    clippy::assert_is_empty,
    reason = "empty assertions read clearly in tests"
)]
mod tests {
    use super::*;
    use crate::rational::Rational;

    fn frame(index: u64, rms: f64) -> AudioFrame {
        let start = Rational::new(i128::from(index), 100).unwrap();
        AudioFrame {
            index,
            start,
            duration: Rational::new(1, 100).unwrap(),
            window_duration: Rational::new(3, 100).unwrap(),
            rms_dbfs: rms,
            peak_dbfs: rms,
        }
    }

    fn thresholds() -> DetectionThresholds {
        DetectionThresholds {
            silence_dbfs: -35.0,
            speech_dbfs: -30.0,
            base_dbfs: -32.5,
            adaptive: false,
        }
    }

    #[test]
    fn detects_one_region_between_speech() {
        let levels = [-6.0, -6.0, -100.0, -100.0, -100.0, -6.0, -6.0];
        let frames: Vec<_> = levels
            .iter()
            .enumerate()
            .map(|(i, &level)| frame(i as u64, level))
            .collect();
        let regions = detect_silences(&frames, thresholds());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].first_frame, 2);
        assert_eq!(regions[0].end_frame, 5);
        // Window of the last silent frame ends at 0.04 + 0.03 = 0.07 s.
        assert_eq!(regions[0].start, Rational::new(2, 100).unwrap());
        assert_eq!(regions[0].end, Rational::new(7, 100).unwrap());
    }

    #[test]
    fn hysteresis_prevents_flapping() {
        // Enter silence at -36 (below -35), then hover between -34 and -33:
        // below the exit level (-30), so the region stays open.
        let levels = [-6.0, -36.0, -34.0, -33.0, -6.0];
        let frames: Vec<_> = levels
            .iter()
            .enumerate()
            .map(|(i, &level)| frame(i as u64, level))
            .collect();
        let regions = detect_silences(&frames, thresholds());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].first_frame, 1);
        assert_eq!(regions[0].end_frame, 4);
    }

    #[test]
    fn region_open_at_eof_is_closed() {
        let frames = vec![frame(0, -6.0), frame(1, -6.0), frame(2, -100.0)];
        let regions = detect_silences(&frames, thresholds());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].end_frame, 3);
    }

    #[test]
    fn no_frames_yields_no_regions() {
        assert!(detect_silences(&[], thresholds()).is_empty());
    }

    #[test]
    fn all_silent_yields_one_region() {
        let frames = vec![frame(0, -100.0), frame(1, -100.0)];
        let regions = detect_silences(&frames, thresholds());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].duration(), Rational::new(4, 100).unwrap());
    }
}

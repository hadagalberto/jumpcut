//! Terminal preview of an analysis result.
//!
//! The preview is intentionally rendered to a `String` instead of printed
//! directly, so the CLI, tests and library consumers share one implementation.
//! It shows the source/output durations, how much was removed and an ASCII
//! loudness sparkline before any rendering happens.

use crate::analysis::{Analysis, sparkline};
use std::fmt::Write;

/// Default width of the loudness sparkline.
pub const DEFAULT_SPARKLINE_WIDTH: usize = 64;

/// Renders a human-readable preview of `analysis`.
///
/// `width` is the sparkline width in characters; zero suppresses the loudness
/// line while keeping the rest of the summary.
#[must_use]
pub fn render_preview(analysis: &Analysis, width: usize) -> String {
    let source = analysis.plan.source_duration.to_f64();
    let removed = analysis.removed_duration().to_f64();
    let output = analysis.output_duration().to_f64();
    let percent = if source > 0.0 {
        removed / source * 100.0
    } else {
        0.0
    };
    let kept = analysis.plan.kept_segments().len();
    let cuts = analysis.plan.cut_segments().len();

    let mut out = String::new();
    let _ = writeln!(out, "source:    {source:.3} s");
    let _ = writeln!(
        out,
        "threshold:{:+.1} dBFS{}",
        analysis.thresholds.base_dbfs,
        if analysis.thresholds.adaptive {
            " (auto)"
        } else {
            ""
        }
    );
    let _ = writeln!(out, "segments:  {kept} kept, {cuts} cut");
    let _ = writeln!(out, "removed:   {removed:.3} s ({percent:.1}%)");
    let _ = writeln!(out, "output:    {output:.3} s");
    if width > 0 {
        let _ = writeln!(out, "loudness:  {}", sparkline(&analysis.frames, width));
    }
    for (index, edit) in analysis.plan.edits.iter().enumerate() {
        let span = edit.span();
        let start = span.start.to_f64();
        let end = span.end.to_f64();
        let (kind, speed) = match edit {
            crate::analysis::Edit::Keep { .. } => ("keep", None),
            crate::analysis::Edit::Cut { .. } => ("cut", None),
            crate::analysis::Edit::SpeedUp { speed, .. } => ("speed", Some(*speed)),
        };
        match speed {
            Some(speed) => {
                let _ = writeln!(
                    out,
                    "  {index:>3}: {kind:<5} {start:8.3} .. {end:8.3} @ {speed:.2}x"
                );
            }
            None => {
                let _ = writeln!(out, "  {index:>3}: {kind:<5} {start:8.3} .. {end:8.3}");
            }
        }
    }
    out
}

/// Renders a preview with the default sparkline width.
#[must_use]
pub fn preview(analysis: &Analysis) -> String {
    render_preview(analysis, DEFAULT_SPARKLINE_WIDTH)
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    reason = "test frame counts are small and exact"
)]
mod tests {
    use super::*;
    use crate::analysis::{
        AnalysisConfig, AudioFrame, FrameConfig, Margins, ThresholdSelection, analyze_frames,
    };
    use crate::rational::{FrameRate, Rational};

    fn analysis(levels: &[f64]) -> Analysis {
        let hop = Rational::new(1, 10).unwrap();
        let frames: Vec<AudioFrame> = levels
            .iter()
            .enumerate()
            .map(|(index, &level)| AudioFrame {
                index: index as u64,
                start: Rational::new(index as i128, 10).unwrap(),
                duration: hop,
                window_duration: hop,
                rms_dbfs: level,
                peak_dbfs: level,
            })
            .collect();
        let config = AnalysisConfig {
            frame: FrameConfig::default(),
            threshold: ThresholdSelection::Absolute(-35.0),
            hysteresis_db: 0.0,
            min_silence: Rational::new(1, 2).unwrap(),
            min_clip: Rational::ZERO,
            margins: Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            merge_gap: Rational::ZERO,
            silent_speed: None,
            frame_rate: Some(FrameRate::FPS_30),
        };
        let duration = Rational::new(levels.len() as i128, 10).unwrap();
        analyze_frames(&frames, &config, duration, 48_000).unwrap()
    }

    #[test]
    fn summary_reports_segments_removed_and_sparkline() {
        let analysis = analysis(&[
            -6.0, -6.0, -6.0, -6.0, -6.0, -6.0, -6.0, -6.0, -6.0, -6.0, -100.0, -100.0, -100.0,
            -100.0, -100.0, -100.0, -6.0, -6.0, -6.0, -6.0,
        ]);
        let text = render_preview(&analysis, 8);
        assert!(text.contains("source:    2.000 s"));
        assert!(text.contains("segments:  2 kept, 1 cut"));
        assert!(text.contains("removed:   0.600 s (30.0%)"));
        assert!(text.contains("output:    1.400 s"));
        assert!(text.contains("loudness:  "));
        assert_eq!(text.lines().count(), 6 + analysis.plan.edits.len());
    }

    #[test]
    fn quiet_and_loud_frames_render_the_full_ramp() {
        let quiet = analysis(&[-140.0; 12]);
        let loud = analysis(&[-3.0; 12]);
        assert!(render_preview(&quiet, 8).contains("loudness:          \n"));
        assert!(render_preview(&loud, 8).contains("loudness:  @@@@@@@@\n"));
    }

    #[test]
    fn zero_width_suppresses_only_the_sparkline() {
        let analysis = analysis(&[-6.0; 12]);
        let text = render_preview(&analysis, 0);
        assert!(!text.contains("loudness:"));
        assert!(text.contains("segments:  1 kept, 0 cut"));
    }
}

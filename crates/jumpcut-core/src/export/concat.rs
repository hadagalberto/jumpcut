//! ffmpeg concat-demuxer list exporter.
//!
//! The list references the original media once per retained span with
//! `inpoint`/`outpoint` directives. It can be fed straight back to
//! `ffmpeg -f concat -safe 0 -i list.txt`. Speeds cannot be expressed by the
//! concat demuxer, so sped-up spans carry an explanatory comment.

use crate::export::{ExportContext, ExportError, concat_path, concat_seconds};
use std::fmt::Write;

/// Renders an ffmpeg concat-demuxer script for `context`.
///
/// # Errors
///
/// Currently infallible, but returns [`ExportError`] for a uniform exporter API.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let mut out = String::from("ffconcat version 1.0\n");
    let file = concat_path(context.source);
    for clip in context.clips() {
        let _ = writeln!(out, "file '{file}'");
        let _ = writeln!(out, "inpoint {}", concat_seconds(clip.source_start));
        let _ = writeln!(out, "outpoint {}", concat_seconds(clip.source_end));
        if let Some(speed) = clip.speed {
            let _ = writeln!(out, "# jumpcut speed: {speed:.6}x");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rational::{FrameRate, Rational};
    use crate::timecode::Timecode;
    use std::path::Path;

    #[test]
    fn lists_one_entry_per_kept_segment() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let list = export(&context).unwrap();
        assert!(list.starts_with("ffconcat version 1.0\n"));
        assert_eq!(list.matches("file '/media/input.mp4'\n").count(), 3);
        assert!(list.contains("file '/media/input.mp4'"));
        assert!(list.contains("inpoint 3.000000\noutpoint 5.000000\n"));
        assert_eq!(list.lines().count(), 1 + 3 * 3);
    }

    #[test]
    fn concat_seconds_use_six_decimal_places() {
        assert_eq!(
            concat_seconds(Rational::new(1001, 30_000).unwrap()),
            "0.033367"
        );
        assert_eq!(concat_seconds(Rational::ZERO), "0.000000");
        // Guard the exact timecode math the exporter relies on.
        let tc = Timecode::new(1_800, FrameRate::NTSC_2997, true).unwrap();
        assert_eq!(tc.format(), "00:01:00;02");
    }
}

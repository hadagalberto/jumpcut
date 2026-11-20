//! CMX3600 EDL exporter.
//!
//! Each retained span becomes one event with source and record in/out points.
//! Event timecodes use `;` separators and `FCM: DROP FRAME` for 29.97/59.94
//! material, so the `00:00:59;29 -> 00:01:00;02` boundary is represented
//! correctly. Sped-up spans carry a comment with their multiplier because
//! CMX3600 has no portable retime field.

use crate::export::{ExportContext, ExportError, PRODUCT_NAME};
use std::fmt::Write;

/// Renders a CMX3600 edit decision list for `context`.
///
/// # Errors
///
/// Returns [`ExportError::Timecode`] for an unrepresentable timeline boundary.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let frame_code = if context.drop_frame {
        "FCM: DROP FRAME"
    } else {
        "FCM: NON-DROP FRAME"
    };
    let mut out = format!(
        "TITLE: {PRODUCT_NAME} - {}\n{frame_code}\n\n",
        context.source_name()
    );
    for clip in context.clips() {
        let source_in = context.timecode(clip.source_start)?.format();
        let source_out = context.timecode(clip.source_end)?.format();
        let record_in = context.timecode(clip.record_start)?.format();
        let record_out = context.timecode(clip.record_end)?.format();
        let _ = writeln!(
            out,
            "{:03}  AX  AA/V  C       {source_in} {source_out} {record_in} {record_out}",
            clip.index + 1
        );
        let _ = writeln!(out, "* FROM CLIP NAME: {}", context.source_name());
        if let Some(speed) = clip.speed {
            let _ = writeln!(out, "* JUMPCUT SPEED: {speed:.6}x");
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
    fn emits_non_drop_events_with_source_and_record_times() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let edl = export(&context).unwrap();
        assert!(edl.contains("FCM: NON-DROP FRAME"));
        assert!(
            edl.contains("001  AX  AA/V  C       00:00:00:00 00:00:02:00 00:00:00:00 00:00:02:00")
        );
        assert!(
            edl.contains("002  AX  AA/V  C       00:00:03:00 00:00:05:00 00:00:02:00 00:00:04:00")
        );
        assert!(!edl.contains(';'));
    }

    #[test]
    fn drop_frame_events_cross_the_one_minute_boundary() {
        let rate = FrameRate::NTSC_2997;
        let last = Timecode::new(1_799, rate, true).unwrap();
        let next = Timecode::new(1_800, rate, true).unwrap();
        assert_eq!(last.format(), "00:00:59;29");
        assert_eq!(next.format(), "00:01:00;02");

        let analysis = crate::export::tests::fixture();
        let context =
            ExportContext::with_timecode(Path::new("/media/input.mp4"), &analysis, rate, true);
        let edl = export(&context).unwrap();
        assert!(edl.contains("FCM: DROP FRAME"));
        assert!(edl.contains(';'));
        assert_eq!(context.frames(Rational::new(60, 1).unwrap()), 1_798);
    }
}

//! Final Cut Pro X FCPXML 1.10 exporter.
//!
//! The document declares a resource format and a single source asset, then a
//! project whose spine holds one `asset-clip` per retained span. All offsets,
//! starts and durations use FCPXML's exact `N/Ds` rational-second notation.
//! A sped-up span carries a `<timeMap>` that maps output time linearly onto
//! source time, which is how FCPXML represents a retime.

use crate::export::{ExportContext, ExportError, file_url, rational_seconds, xml_escape};
use std::fmt::Write;

/// Renders the FCPXML 1.10 document for `context`.
///
/// The format resource describes the probed video stream rather than assuming
/// a 1080p30 source; unknown dimensions or rates use Apple's
/// `FFVideoFormatRateUndefined` name.
///
/// # Errors
///
/// Currently infallible, but returns [`ExportError`] for a uniform exporter API.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let format_name = context.video.fcp_format_name(context.frame_rate);
    let color_space = context.video.fcp_color_space_or_default();
    let width = context.video.width;
    let height = context.video.height;
    let tc_format = if context.drop_frame { "DF" } else { "NDF" };
    let source_name = xml_escape(&context.source_name());
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<!DOCTYPE fcpxml>\n");
    out.push_str("<fcpxml version=\"1.10\">\n");
    out.push_str("  <resources>\n");
    let _ = writeln!(
        out,
        "    <format id=\"rFmt1\" name=\"{format_name}\" frameDuration=\"{}\" width=\"{width}\" height=\"{height}\" colorSpace=\"{color_space}\"/>",
        rational_seconds(context.frame_duration())
    );
    let _ = writeln!(
        out,
        "    <asset id=\"rAsset1\" name=\"{source_name}\" src=\"{}\" duration=\"{}\" hasVideo=\"1\" hasAudio=\"1\">",
        xml_escape(&file_url(context.source)),
        rational_seconds(context.analysis.plan.source_duration)
    );
    out.push_str("      <format ref=\"rFmt1\"/>\n");
    out.push_str("    </asset>\n");
    out.push_str("  </resources>\n");
    out.push_str("  <library>\n");
    out.push_str("    <event name=\"jumpcut\">\n");
    let _ = writeln!(out, "      <project name=\"{source_name} - jumpcut\">");
    let _ = writeln!(
        out,
        "        <sequence duration=\"{}\" format=\"rFmt1\">",
        rational_seconds(context.output_duration())
    );
    out.push_str("          <spine>\n");
    for clip in context.clips() {
        let _ = writeln!(
            out,
            "            <asset-clip ref=\"rAsset1\" offset=\"{}\" start=\"{}\" duration=\"{}\" name=\"{source_name} {}\" tcFormat=\"{tc_format}\">",
            rational_seconds(clip.record_start),
            rational_seconds(clip.source_start),
            rational_seconds(clip.record_duration()),
            clip.index + 1
        );
        if let Some(speed) = clip.speed {
            out.push_str("              <timeMap>\n");
            out.push_str(
                "                <timePt time=\"0/1s\" value=\"0/1s\" interp=\"linear\"/>\n",
            );
            let _ = writeln!(
                out,
                "                <timePt time=\"{}\" value=\"{}\" interp=\"linear\"/>",
                rational_seconds(clip.record_duration()),
                rational_seconds(clip.source_duration())
            );
            out.push_str("              </timeMap>\n");
            let _ = writeln!(out, "              <!-- jumpcut speed: {speed:.6}x -->");
        }
        out.push_str("            </asset-clip>\n");
    }
    out.push_str("          </spine>\n");
    out.push_str("        </sequence>\n");
    out.push_str("      </project>\n");
    out.push_str("    </event>\n");
    out.push_str("  </library>\n");
    out.push_str("</fcpxml>\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportFormat, VideoInfo};
    use crate::rational::{FrameRate, Rational};
    use std::path::Path;

    fn video_context<'a>(
        analysis: &'a crate::analysis::Analysis,
        width: u32,
        height: u32,
        rate: FrameRate,
        color_space: Option<&str>,
        field_order: Option<&str>,
    ) -> ExportContext<'a> {
        let video = VideoInfo {
            width,
            height,
            color_space: color_space.map(str::to_owned),
            color_primaries: None,
            color_transfer: None,
            field_order: field_order.map(str::to_owned),
        };
        ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            analysis,
            rate,
            rate.drop_frames_per_minute().is_some(),
        )
        .with_video(video)
    }

    #[test]
    fn contains_asset_format_and_spine_clips() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("version=\"1.10\""));
        assert!(xml.contains("<!DOCTYPE fcpxml>"));
        assert!(xml.contains("<spine>"));
        assert!(xml.contains("frameDuration=\"1/30s\""));
        assert_eq!(xml.matches("<asset-clip ").count(), 3);
        assert_eq!(
            crate::export::export(&context, ExportFormat::Fcpxml).unwrap(),
            xml
        );
    }

    #[test]
    fn uses_drop_frame_timecode_format_and_exact_frame_duration() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::NTSC_2997,
            true,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("tcFormat=\"DF\""));
        assert!(xml.contains("frameDuration=\"1001/30000s\""));
        assert_eq!(rational_seconds(context.frame_duration()), "1001/30000s");
        assert_eq!(
            Rational::new(1001, 30_000).unwrap(),
            context.frame_duration()
        );
    }

    #[test]
    fn uses_probed_dimensions_rate_and_color_space() {
        let analysis = crate::export::tests::fixture();
        let context = video_context(
            &analysis,
            320,
            240,
            FrameRate::NTSC_2997,
            Some("bt709"),
            None,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("name=\"FFVideoFormatRateUndefined\""));
        assert!(xml.contains("width=\"320\" height=\"240\""));
        assert!(xml.contains("frameDuration=\"1001/30000s\""));
        assert!(xml.contains("colorSpace=\"1-1-1 (Rec. 709)\""));
        assert!(!xml.contains("FFVideoFormat1080p30"));

        let hd = video_context(&analysis, 1920, 1080, FrameRate::NTSC_23976, None, None);
        let xml = export(&hd).unwrap();
        assert!(xml.contains("name=\"FFVideoFormat1080p2398\""));
        assert!(xml.contains("frameDuration=\"1001/24000s\""));
    }

    #[test]
    fn snapshot_format_320x240_2997() {
        let analysis = crate::export::tests::fixture();
        let context = video_context(
            &analysis,
            320,
            240,
            FrameRate::NTSC_2997,
            Some("bt709"),
            None,
        );
        insta::assert_snapshot!("fcpxml_320x240_2997", export(&context).unwrap());
    }

    #[test]
    fn snapshot_format_1920x1080_23976() {
        let analysis = crate::export::tests::fixture();
        let context = video_context(&analysis, 1920, 1080, FrameRate::NTSC_23976, None, None);
        insta::assert_snapshot!("fcpxml_1920x1080_23976", export(&context).unwrap());
    }

    #[test]
    fn snapshot_format_3840x2160_60() {
        let analysis = crate::export::tests::fixture();
        let context = video_context(
            &analysis,
            3840,
            2160,
            FrameRate::FPS_60,
            Some("bt2020nc"),
            None,
        );
        insta::assert_snapshot!("fcpxml_3840x2160_60", export(&context).unwrap());
    }

    #[test]
    fn snapshot_format_1080x1920_30() {
        let analysis = crate::export::tests::fixture();
        let context = video_context(&analysis, 1080, 1920, FrameRate::FPS_30, None, None);
        insta::assert_snapshot!("fcpxml_1080x1920_30", export(&context).unwrap());
    }
}

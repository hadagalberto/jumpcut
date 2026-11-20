//! Adobe Premiere xmeml v4 exporter.
//!
//! The document follows Apple's *Final Cut Pro 7 XML Interchange Format*
//! (xmeml v4): `<sequence>` owns a `<media>` with a `<video>` and an `<audio>`
//! section; each section owns a `<track>` of `<clipitem>`s. Every `<rate>` is a
//! structure with `<timebase>` and `<ntsc>` subelements, all timing values are
//! whole frames at the export frame rate, and the original file is defined
//! once (with its full media duration) and referenced by id from every clip.
//!
//! Sources: Apple's element reference
//! <https://developer.apple.com/library/archive/documentation/AppleApplications/Reference/FinalCutPro_XML/Elements/Elements.html>
//! and the v4 DTD in
//! <https://developer.apple.com/library/archive/documentation/AppleApplications/Reference/FinalCutPro_XML/DTD/DTD.html>.
//! The independent hierarchy and value checks live in `tests/nle_schema.rs`.

use crate::export::{ExportContext, ExportError, PRODUCT_NAME, file_url, xml_escape};
use crate::rational::Rational;
use std::fmt::Write;

/// Id shared by the single media definition and all clip references.
const MEDIA_ID: &str = "jumpcut-media-1";

/// Renders an xmeml v4 sequence for `context`.
///
/// Dimensions come from the probed video stream; `ntsc` reflects whether the
/// source rate is NTSC, independently of the drop-frame timecode preference.
///
/// # Errors
///
/// Returns [`ExportError::Timecode`] for an unrepresentable timeline boundary.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let rate = context.frame_rate.nominal_fps();
    let ntsc = if context.frame_rate.is_ntsc() {
        "TRUE"
    } else {
        "FALSE"
    };
    let width = context.video.width;
    let height = context.video.height;
    let name = xml_escape(&context.source_name());
    let total_frames = context.frames(context.output_duration());
    // The source timecode belongs to the media file, not to a particular clip.
    // It is emitted once, at frame zero, in the sequence's drop-frame mode.
    let source_tc = context.timecode(Rational::ZERO)?.format();
    let tc_format = if context.drop_frame { "DF" } else { "NDF" };

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<xmeml version=\"4\">\n");
    let _ = writeln!(
        out,
        "  <sequence id=\"jumpcut-sequence\" name=\"{name} - {PRODUCT_NAME}\">"
    );
    let _ = writeln!(out, "    <name>{name} - {PRODUCT_NAME}</name>");
    let _ = writeln!(out, "    <duration>{total_frames}</duration>");
    out.push_str(&rate_block("    ", rate, ntsc));
    out.push_str("    <media>\n");

    out.push_str("      <video>\n");
    out.push_str("        <format>\n");
    out.push_str("          <samplecharacteristics>\n");
    out.push_str(&rate_block("            ", rate, ntsc));
    let _ = writeln!(out, "            <width>{width}</width>");
    let _ = writeln!(out, "            <height>{height}</height>");
    out.push_str("          </samplecharacteristics>\n");
    out.push_str("        </format>\n");
    out.push_str("        <track>\n");
    out.push_str(&clip_items(
        context, false, rate, ntsc, &name, &source_tc, tc_format,
    ));
    out.push_str("        </track>\n");
    out.push_str("      </video>\n");

    out.push_str("      <audio>\n");
    out.push_str("        <format>\n");
    out.push_str("          <samplecharacteristics>\n");
    out.push_str("            <depth>16</depth>\n");
    out.push_str("            <samplerate>48000</samplerate>\n");
    out.push_str("          </samplecharacteristics>\n");
    out.push_str("        </format>\n");
    out.push_str("        <track>\n");
    out.push_str(&clip_items(
        context, true, rate, ntsc, &name, &source_tc, tc_format,
    ));
    out.push_str("        </track>\n");
    out.push_str("      </audio>\n");

    out.push_str("    </media>\n");
    out.push_str("  </sequence>\n");
    out.push_str("</xmeml>\n");
    Ok(out)
}

/// Apple's v4 `rate` element: a timebase plus an NTSC flag, never a bare number.
fn rate_block(indent: &str, rate: u32, ntsc: &str) -> String {
    format!(
        "{indent}<rate>\n{indent}  <timebase>{rate}</timebase>\n{indent}  <ntsc>{ntsc}</ntsc>\n{indent}</rate>\n"
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "all values describe the same sequence"
)]
fn clip_items(
    context: &ExportContext<'_>,
    audio: bool,
    rate: u32,
    ntsc: &str,
    name: &str,
    source_tc: &str,
    tc_format: &str,
) -> String {
    let width = context.video.width;
    let height = context.video.height;
    // Media length in frames, not the out point of the first clip.
    let media_frames = context.frames(context.analysis.plan.source_duration);
    let url = xml_escape(&file_url(context.source));
    let mut out = String::new();
    for clip in context.clips() {
        let source_in = context.frames(clip.source_start);
        let source_out = context.frames(clip.source_end);
        let record_in = context.frames(clip.record_start);
        let record_out = context.frames(clip.record_end);
        let duration = record_out - record_in;
        let kind = if audio { "audio" } else { "video" };
        let id = format!("jumpcut-{kind}-{}", clip.index);
        let linked = if audio {
            format!("jumpcut-video-{}", clip.index)
        } else {
            format!("jumpcut-audio-{}", clip.index)
        };
        let _ = writeln!(out, "          <clipitem id=\"{id}\">");
        let _ = writeln!(out, "            <name>{name} {}</name>", clip.index + 1);
        let _ = writeln!(out, "            <duration>{duration}</duration>");
        out.push_str(&rate_block("            ", rate, ntsc));
        let _ = writeln!(out, "            <start>{record_in}</start>");
        let _ = writeln!(out, "            <end>{record_out}</end>");
        let _ = writeln!(out, "            <in>{source_in}</in>");
        let _ = writeln!(out, "            <out>{source_out}</out>");
        if let Some(speed) = clip.speed {
            let _ = writeln!(out, "            <!-- jumpcut speed: {speed:.6}x -->");
        }
        let _ = writeln!(
            out,
            "            <link><linkclipref>{linked}</linkclipref></link>"
        );

        // Define the media once (in the first video clip); all other clips
        // reference that definition by id.
        if !audio && clip.index == 0 {
            let _ = writeln!(out, "            <file id=\"{MEDIA_ID}\">");
            let _ = writeln!(out, "              <name>{name}</name>");
            let _ = writeln!(out, "              <pathurl>{url}</pathurl>");
            let _ = writeln!(out, "              <duration>{media_frames}</duration>");
            out.push_str(&rate_block("              ", rate, ntsc));
            let _ = writeln!(out, "              <timecode>");
            let _ = writeln!(out, "                <string>{source_tc}</string>");
            out.push_str(&rate_block("                ", rate, ntsc));
            let _ = writeln!(
                out,
                "                <displayformat>{tc_format}</displayformat>"
            );
            let _ = writeln!(out, "              </timecode>");
            let _ = writeln!(out, "              <media>");
            let _ = writeln!(
                out,
                "                <video><duration>{media_frames}</duration><width>{width}</width><height>{height}</height></video>"
            );
            let _ = writeln!(
                out,
                "                <audio><duration>{media_frames}</duration><numOutputChannels>2</numOutputChannels><format>int</format><sampleRate>48000</sampleRate></audio>"
            );
            let _ = writeln!(out, "              </media>");
            let _ = writeln!(out, "            </file>");
        } else {
            let _ = writeln!(out, "            <file id=\"{MEDIA_ID}\"/>");
        }
        let _ = writeln!(out, "          </clipitem>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::VideoInfo;
    use crate::rational::FrameRate;
    use std::path::Path;

    #[test]
    fn contains_sequence_tracks_and_clipitems() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("<xmeml version=\"4\">"));
        assert!(xml.contains("<sequence "));
        assert_eq!(xml.matches("<clipitem ").count(), 6);
        assert!(xml.contains("<in>0</in>"));
        assert!(xml.contains("<out>60</out>"));
        assert!(xml.contains("file:///media/input.mp4"));
    }

    #[test]
    fn drop_frame_sequence_uses_ntsc_flag_and_source_timecode() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::NTSC_2997,
            true,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("<ntsc>TRUE</ntsc>"));
        // One source timecode describes frame 0 of the media file. The old
        // exporter also wrote a per-clip record timecode (00:00:02;00 here),
        // which is not a property of the referenced file. See nle_schema.rs.
        assert!(xml.contains("<string>00:00:00;00</string>"));
        assert!(!xml.contains("<string>00:00:02;00</string>"));
        assert!(xml.contains("<displayformat>DF</displayformat>"));
    }

    #[test]
    fn sample_characteristics_use_probed_dimensions() {
        let analysis = crate::export::tests::fixture();
        let video = VideoInfo {
            width: 320,
            height: 240,
            ..VideoInfo::DEFAULT
        };
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        )
        .with_video(video);
        let xml = export(&context).unwrap();
        assert!(xml.contains("<width>320</width>"));
        assert!(xml.contains("<height>240</height>"));
        // One sequence format plus one media definition: the old exporter
        // repeated the dimensions inside every per-clip <file>, which the
        // documented hierarchy does not allow. See nle_schema.rs.
        assert_eq!(xml.matches("<width>320</width>").count(), 2);
        assert!(!xml.contains("<width>1920</width>"));
    }

    #[test]
    fn ntsc_flag_and_timebase_follow_frame_rate_not_drop_frame_preference() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::NTSC_2997,
            false,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("<ntsc>TRUE</ntsc>"));
        assert!(xml.contains("<timebase>30</timebase>"));

        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::NTSC_23976,
            false,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("<ntsc>TRUE</ntsc>"));
        assert!(xml.contains("<timebase>24</timebase>"));
    }

    #[test]
    fn file_duration_is_the_full_media_length() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let xml = export(&context).unwrap();
        // 7 seconds of source at 30 fps = 210 frames; the first clip's out
        // point (60) is not the file length.
        // Sequence duration is the 5 s output (150 frames); the file and its
        // video/audio media descriptors carry the 7 s source (210 frames).
        assert_eq!(xml.matches("<duration>210</duration>").count(), 3);
        assert_eq!(xml.matches("<file id=\"jumpcut-media-1\"").count(), 6);
        assert_eq!(xml.matches("<file id=\"jumpcut-media-1\"/>").count(), 5);
    }
}

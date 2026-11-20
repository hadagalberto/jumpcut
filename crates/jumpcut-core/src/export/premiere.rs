//! Adobe Premiere xmeml v4 exporter.
//!
//! Premiere counts clips in whole frames, so source and record boundaries are
//! converted through [`Timecode`] at the export frame rate (including
//! drop-frame labels for 29.97/59.94). The exported sequence has one video
//! track and one audio track, both referencing the original media file.

use crate::export::{ExportContext, ExportError, PRODUCT_NAME, file_url, xml_escape};
use std::fmt::Write;

/// Default video width written into the sequence format.
const WIDTH: u32 = 1920;
/// Default video height written into the sequence format.
const HEIGHT: u32 = 1080;

/// Renders an xmeml v4 sequence for `context`.
///
/// # Errors
///
/// Returns [`ExportError::Timecode`] for an unrepresentable timeline boundary.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let rate = context.frame_rate.nominal_fps();
    let ntsc = if context.drop_frame { "TRUE" } else { "FALSE" };
    let name = xml_escape(&context.source_name());
    let total_frames = context.frames(context.output_duration());
    let clips = context.clips();

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<xmeml version=\"4\">\n");
    let _ = writeln!(
        out,
        "  <sequence id=\"jumpcut-sequence\" name=\"{name} - {PRODUCT_NAME}\">"
    );
    let _ = writeln!(out, "    <duration>{total_frames}</duration>");
    let _ = writeln!(out, "    <rate>{rate}</rate>");
    let _ = writeln!(out, "    <timebase>{rate}</timebase>");
    let _ = writeln!(out, "    <ntsc>{ntsc}</ntsc>");
    out.push_str("    <format>\n");
    out.push_str("      <samplecharacteristics>\n");
    let _ = writeln!(
        out,
        "        <rate><timebase>{rate}</timebase><ntsc>{ntsc}</ntsc></rate>"
    );
    let _ = writeln!(out, "        <width>{WIDTH}</width>");
    let _ = writeln!(out, "        <height>{HEIGHT}</height>");
    out.push_str("      </samplecharacteristics>\n");
    out.push_str("    </format>\n");

    out.push_str("    <track>\n");
    for clip in &clips {
        out.push_str(&clip_item(context, clip.index, "clipitem", false)?);
    }
    out.push_str("    </track>\n");
    out.push_str("    <track>\n");
    for clip in &clips {
        out.push_str(&clip_item(context, clip.index, "clipitem", true)?);
    }
    out.push_str("    </track>\n");
    out.push_str("  </sequence>\n");
    out.push_str("</xmeml>\n");
    Ok(out)
}

fn clip_item(
    context: &ExportContext<'_>,
    index: usize,
    tag: &str,
    audio: bool,
) -> Result<String, ExportError> {
    let clip = context
        .clips()
        .into_iter()
        .find(|candidate| candidate.index == index)
        .expect("clip index came from this context");
    let rate = context.frame_rate.nominal_fps();
    let source_in = context.frames(clip.source_start);
    let source_out = context.frames(clip.source_end);
    let record_in = context.frames(clip.record_start);
    let record_out = context.frames(clip.record_end);
    let duration = record_out - record_in;
    let source_tc = context.timecode(clip.source_start)?.format();
    let record_tc = context.timecode(clip.record_start)?.format();
    let kind = if audio { "audio" } else { "video" };
    let id = format!("jumpcut-{kind}-{index}");
    let name = xml_escape(&context.source_name());
    let mut out = String::new();
    let _ = writeln!(out, "      <{tag} id=\"{id}\">");
    let _ = writeln!(out, "        <name>{name} {}</name>", index + 1);
    let _ = writeln!(out, "        <duration>{duration}</duration>");
    let _ = writeln!(out, "        <rate>{rate}</rate>");
    let _ = writeln!(out, "        <start>{record_in}</start>");
    let _ = writeln!(out, "        <end>{record_out}</end>");
    let _ = writeln!(out, "        <in>{source_in}</in>");
    let _ = writeln!(out, "        <out>{source_out}</out>");
    if let Some(speed) = clip.speed {
        let _ = writeln!(out, "        <!-- jumpcut speed: {speed:.6}x -->");
    }
    let _ = writeln!(out, "        <file id=\"jumpcut-media-{index}\">");
    let _ = writeln!(out, "          <name>{name}</name>");
    let _ = writeln!(
        out,
        "          <pathurl>{}</pathurl>",
        xml_escape(&file_url(context.source))
    );
    let _ = writeln!(out, "          <duration>{source_out}</duration>");
    out.push_str("          <media>\n");
    if audio {
        out.push_str("            <audio><numOutputChannels>2</numOutputChannels><format>int</format><sampleRate>48000</sampleRate></audio>\n");
    } else {
        let _ = writeln!(
            out,
            "            <video><duration>{source_out}</duration><width>{WIDTH}</width><height>{HEIGHT}</height></video>"
        );
    }
    out.push_str("          </media>\n");
    let _ = writeln!(out, "          <rate><timebase>{rate}</timebase></rate>");
    let _ = writeln!(
        out,
        "          <timecode><string>{source_tc}</string><frameRate>{rate}</frameRate></timecode>"
    );
    let _ = writeln!(
        out,
        "          <timecode><string>{record_tc}</string><frameRate>{rate}</frameRate></timecode>"
    );
    out.push_str("        </file>\n");
    let _ = writeln!(out, "      </{tag}>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn drop_frame_sequence_uses_ntsc_flag_and_semicolon_timecode() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::NTSC_2997,
            true,
        );
        let xml = export(&context).unwrap();
        assert!(xml.contains("<ntsc>TRUE</ntsc>"));
        assert!(xml.contains("00:00:02;00"));
    }
}

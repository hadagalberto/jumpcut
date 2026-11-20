//! Timeline exporters for NLEs and ffmpeg.
//!
//! Every exporter consumes the same [`ExportContext`] built from an
//! [`Analysis`], so all formats describe exactly the same edit decisions.
//! Source and record positions stay as exact [`Rational`] values until the
//! final formatting step; formats that count frames (Premiere, EDL, OTIO)
//! convert through [`Timecode`] with the correct NTSC drop-frame mode.

use std::fmt;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;
use thiserror::Error;

use crate::analysis::Analysis;
use crate::rational::{FrameRate, Rational};
use crate::timecode::{Timecode, TimecodeError};

pub mod concat;
pub mod edl;
pub mod fcpxml;
pub mod json;
pub mod otio;
pub mod premiere;

/// Errors produced while formatting an edit timeline.
#[derive(Debug, Error)]
pub enum ExportError {
    /// A timeline boundary could not be represented as a timecode.
    #[error(transparent)]
    Timecode(#[from] TimecodeError),
    /// Formatted JSON could not be serialised.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// The timeline formats jumpcut can write.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// Final Cut Pro X FCPXML 1.10.
    Fcpxml,
    /// Adobe Premiere xmeml v4.
    Premiere,
    /// CMX3600 EDL (with drop-frame support).
    Edl,
    /// `OpenTimelineIO` JSON.
    Otio,
    /// ffmpeg concat demuxer list.
    Concat,
    /// jumpcut's machine-readable JSON timeline.
    Json,
}

impl ExportFormat {
    /// All formats in their canonical CLI order.
    pub const ALL: [Self; 6] = [
        Self::Fcpxml,
        Self::Premiere,
        Self::Edl,
        Self::Otio,
        Self::Concat,
        Self::Json,
    ];

    /// The canonical CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fcpxml => "fcpxml",
            Self::Premiere => "premiere",
            Self::Edl => "edl",
            Self::Otio => "otio",
            Self::Concat => "concat",
            Self::Json => "json",
        }
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ExportFormat {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "fcpxml" | "fcp" => Ok(Self::Fcpxml),
            "premiere" | "xmeml" => Ok(Self::Premiere),
            "edl" | "cmx3600" => Ok(Self::Edl),
            "otio" => Ok(Self::Otio),
            "concat" => Ok(Self::Concat),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unknown export format `{other}`; expected one of: {}",
                {
                    Self::ALL
                        .iter()
                        .map(|format| format.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )),
        }
    }
}

/// One retained clip in the exported timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineClip {
    /// Zero-based clip index in record order.
    pub index: usize,
    /// In point in the source timeline (exact seconds).
    pub source_start: Rational,
    /// Out point in the source timeline (exact seconds).
    pub source_end: Rational,
    /// In point in the rendered timeline (exact seconds).
    pub record_start: Rational,
    /// Out point in the rendered timeline (exact seconds).
    pub record_end: Rational,
    /// Playback speed multiplier when this clip is sped up.
    pub speed: Option<f64>,
}

impl TimelineClip {
    /// Length of this clip in the rendered timeline.
    #[must_use]
    pub fn record_duration(&self) -> Rational {
        self.record_end - self.record_start
    }

    /// Length of source audio/video consumed by this clip.
    #[must_use]
    pub fn source_duration(&self) -> Rational {
        self.source_end - self.source_start
    }
}

/// Immutable input shared by all exporters.
#[derive(Clone, Debug)]
pub struct ExportContext<'a> {
    /// Media file referenced by the exported timeline.
    pub source: &'a Path,
    /// Analysis whose edit plan is exported.
    pub analysis: &'a Analysis,
    /// Frame rate used for timecode conversion.
    pub frame_rate: FrameRate,
    /// Whether timecodes count in drop-frame mode.
    pub drop_frame: bool,
}

impl<'a> ExportContext<'a> {
    /// Builds a context, defaulting to non-drop timecode on integer rates and
    /// drop-frame timecode on NTSC 29.97 / 59.94 rates.
    #[must_use]
    pub fn new(source: &'a Path, analysis: &'a Analysis) -> Self {
        let frame_rate = analysis.plan.frame_rate.unwrap_or(FrameRate::FPS_30);
        Self {
            source,
            analysis,
            frame_rate,
            drop_frame: frame_rate.drop_frames_per_minute().is_some(),
        }
    }

    /// Builds a context with explicit timecode settings.
    #[must_use]
    pub const fn with_timecode(
        source: &'a Path,
        analysis: &'a Analysis,
        frame_rate: FrameRate,
        drop_frame: bool,
    ) -> Self {
        Self {
            source,
            analysis,
            frame_rate,
            drop_frame,
        }
    }

    /// All retained clips with their source and record positions.
    #[must_use]
    pub fn clips(&self) -> Vec<TimelineClip> {
        let mut cursor = Rational::ZERO;
        let mut clips = Vec::new();
        for (index, edit) in self
            .analysis
            .plan
            .edits
            .iter()
            .filter(|edit| edit.is_kept())
            .enumerate()
        {
            let span = edit.span();
            let speed = match edit {
                crate::analysis::Edit::SpeedUp { speed, .. } => Some(*speed),
                _ => None,
            };
            let played = match speed {
                Some(speed) => span.duration() / speed_ratio(speed),
                None => span.duration(),
            };
            clips.push(TimelineClip {
                index,
                source_start: span.start,
                source_end: span.end,
                record_start: cursor,
                record_end: cursor + played,
                speed,
            });
            cursor += played;
        }
        clips
    }

    /// File name of the referenced media, or `input` when unavailable.
    #[must_use]
    pub fn source_name(&self) -> String {
        self.source.file_name().map_or_else(
            || "input".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Duration of the rendered timeline.
    #[must_use]
    pub fn output_duration(&self) -> Rational {
        self.analysis.output_duration()
    }

    /// Formats an exact duration as NLE timecode.
    ///
    /// # Errors
    ///
    /// Returns [`ExportError::Timecode`] for a negative or unrepresentable time.
    pub fn timecode(&self, seconds: Rational) -> Result<Timecode, ExportError> {
        Ok(Timecode::from_seconds(
            seconds,
            self.frame_rate,
            self.drop_frame,
        )?)
    }

    /// Formats an exact duration as a frame count at the export frame rate.
    #[must_use]
    pub fn frames(&self, seconds: Rational) -> u64 {
        u64::try_from(seconds.frame_count(self.frame_rate)).unwrap_or(0)
    }

    /// Duration of one frame as rational seconds (`1001/30000s` for 29.97).
    #[must_use]
    pub fn frame_duration(&self) -> Rational {
        Rational::ONE / self.frame_rate.as_rational()
    }
}

fn speed_ratio(speed: f64) -> Rational {
    Rational::parse(&format!("{speed}")).unwrap_or(Rational::ONE)
}

/// Formats exact seconds as the `N/Ds` notation required by FCPXML.
#[must_use]
pub fn rational_seconds(seconds: Rational) -> String {
    format!("{}/{}s", seconds.num(), seconds.den())
}

/// Escapes text for use in an XML attribute or element body.
#[must_use]
pub fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Resolves `path` to an absolute path without requiring it to exist.
#[must_use]
pub fn absolute_path(path: &Path) -> PathBuf {
    // Treat POSIX-style rooted paths as absolute on every platform so exported
    // URLs and snapshots stay stable in cross-platform test fixtures.
    if path.is_absolute() || path.to_string_lossy().starts_with('/') {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

/// Converts a path to a percent-encoded `file://` URL with forward slashes.
#[must_use]
pub fn file_url(path: &Path) -> String {
    let absolute = absolute_path(path);
    let text = absolute.to_string_lossy().replace('\\', "/");
    let with_root = if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    };
    let mut url = String::from("file://");
    for byte in with_root.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(char::from(byte));
            }
            _ => {
                let _ = write!(url, "%{byte:02X}");
            }
        }
    }
    url
}

/// Escapes a path for a quoted ffmpeg concat-demuxer `file` directive.
#[must_use]
pub fn concat_path(path: &Path) -> String {
    let absolute = absolute_path(path);
    absolute
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

/// Formats a duration with the precision ffmpeg's concat demuxer accepts.
#[must_use]
pub fn concat_seconds(seconds: Rational) -> String {
    format!("{:.6}", seconds.to_f64())
}

/// Prefix used by generated EDL titles and timeline names.
pub const PRODUCT_NAME: &str = "jumpcut";

/// Dispatches to the exporter for `format`.
///
/// # Errors
///
/// Propagates timecode conversion errors from the selected exporter.
pub fn export(context: &ExportContext<'_>, format: ExportFormat) -> Result<String, ExportError> {
    match format {
        ExportFormat::Fcpxml => fcpxml::export(context),
        ExportFormat::Premiere => premiere::export(context),
        ExportFormat::Edl => edl::export(context),
        ExportFormat::Otio => otio::export(context),
        ExportFormat::Concat => concat::export(context),
        ExportFormat::Json => json::export(context),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        AnalysisConfig, Dbfs, DetectionThresholds, FrameConfig, Margins, ThresholdSelection,
    };
    use crate::rational::FrameRate;

    /// Deterministic 7-second plan: keep 0..2, 3..5 and 6..7 at 30 fps.
    pub(crate) fn fixture() -> Analysis {
        let hop = Rational::new(1, 10).unwrap();
        let levels = [
            (-6.0_f64, 20_usize),
            (-100.0, 10),
            (-6.0, 20),
            (-100.0, 10),
            (-6.0, 10),
        ];
        let mut frames = Vec::new();
        let mut index = 0_u64;
        for (level, count) in levels {
            for _ in 0..count {
                let start = Rational::new(i128::from(index) * hop.num(), hop.den()).unwrap();
                frames.push(crate::analysis::AudioFrame {
                    index,
                    start,
                    duration: hop,
                    window_duration: hop,
                    rms_dbfs: level,
                    peak_dbfs: level,
                });
                index += 1;
            }
        }
        let config = AnalysisConfig {
            frame: FrameConfig::default(),
            threshold: ThresholdSelection::Absolute(Dbfs(-35.0).0),
            hysteresis_db: 0.0,
            min_silence: Rational::new(1, 2).unwrap(),
            min_clip: Rational::new(1, 10).unwrap(),
            margins: Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            merge_gap: Rational::ZERO,
            silent_speed: None,
            frame_rate: Some(FrameRate::FPS_30),
        };
        crate::analysis::analyze_frames(&frames, &config, Rational::new(7, 1).unwrap(), 48_000)
            .unwrap()
    }

    fn context<'a>(source: &'a Path, analysis: &'a Analysis) -> ExportContext<'a> {
        ExportContext::with_timecode(source, analysis, FrameRate::FPS_30, false)
    }

    #[test]
    fn fixture_has_expected_clips() {
        let analysis = fixture();
        let context = context(Path::new("/media/input.mp4"), &analysis);
        let clips = context.clips();
        assert_eq!(clips.len(), 3);
        assert_eq!(clips[0].source_start, Rational::ZERO);
        assert_eq!(clips[0].source_end, Rational::new(2, 1).unwrap());
        assert_eq!(clips[1].record_start, Rational::new(2, 1).unwrap());
        assert_eq!(clips[2].record_end, Rational::new(5, 1).unwrap());
        assert_eq!(context.output_duration(), Rational::new(5, 1).unwrap());
    }

    #[test]
    fn formats_exact_seconds_and_urls() {
        assert_eq!(
            rational_seconds(Rational::new(1001, 30_000).unwrap()),
            "1001/30000s"
        );
        assert_eq!(rational_seconds(Rational::new(5, 1).unwrap()), "5/1s");
        assert_eq!(
            file_url(Path::new("/media/my clip.mp4")),
            "file:///media/my%20clip.mp4"
        );
        assert_eq!(
            concat_path(Path::new("/media/a'b.mp4")),
            "/media/a'\\''b.mp4"
        );
    }

    #[test]
    fn parses_export_formats() {
        for format in ExportFormat::ALL {
            assert_eq!(format.as_str().parse::<ExportFormat>().unwrap(), format);
        }
        assert_eq!("FCP".parse::<ExportFormat>().unwrap(), ExportFormat::Fcpxml);
        assert!("nle".parse::<ExportFormat>().is_err());
    }

    #[test]
    fn exports_every_format_without_error() {
        let analysis = fixture();
        let context = context(Path::new("/media/input.mp4"), &analysis);
        for format in ExportFormat::ALL {
            let text = export(&context, format).unwrap();
            assert!(!text.is_empty(), "empty output for {format}");
        }
    }

    #[test]
    fn fixture_thresholds_are_known() {
        assert_eq!(
            fixture().thresholds,
            DetectionThresholds {
                silence_dbfs: -35.0,
                speech_dbfs: -35.0,
                base_dbfs: -35.0,
                adaptive: false,
            }
        );
    }

    #[test]
    fn xml_escaping_covers_all_markup_characters() {
        assert_eq!(
            xml_escape("a&b<c>\"d\"'e'"),
            "a&amp;b&lt;c&gt;&quot;d&quot;&apos;e&apos;"
        );
    }

    fn snapshot(format: ExportFormat) -> String {
        let analysis = fixture();
        let context = context(Path::new("/media/input.mp4"), &analysis);
        export(&context, format).unwrap()
    }

    #[test]
    fn snapshot_fcpxml() {
        insta::assert_snapshot!("fcpxml", snapshot(ExportFormat::Fcpxml));
    }

    #[test]
    fn snapshot_premiere() {
        insta::assert_snapshot!("premiere", snapshot(ExportFormat::Premiere));
    }

    #[test]
    fn snapshot_edl() {
        insta::assert_snapshot!("edl", snapshot(ExportFormat::Edl));
    }

    #[test]
    fn snapshot_otio() {
        insta::assert_snapshot!("otio", snapshot(ExportFormat::Otio));
    }

    #[test]
    fn snapshot_concat() {
        insta::assert_snapshot!("concat", snapshot(ExportFormat::Concat));
    }

    #[test]
    fn snapshot_json() {
        insta::assert_snapshot!("json", snapshot(ExportFormat::Json));
    }

    #[test]
    fn snapshot_preview() {
        let analysis = fixture();
        insta::assert_snapshot!("preview", crate::preview::render_preview(&analysis, 32));
    }
}

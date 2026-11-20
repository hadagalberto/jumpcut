//! ffmpeg rendering of an edit plan.
//!
//! Two strategies are available:
//!
//! * [`RenderStrategy::FilterScript`] builds one `filter_complex` graph with
//!   `trim`/`atrim` and `concat`. It is frame accurate, supports `atempo`
//!   retimes and avoids temporary files, but grows with the segment count.
//! * [`RenderStrategy::ConcatDemuxer`] joins trimmed segments through an ffmpeg
//!   concat list. Re-encode mode first renders each span accurately into a
//!   temporary file; stream-copy mode references the original media with
//!   `inpoint`/`outpoint` pairs (and warns that cuts may snap to keyframes).
//!
//! Small plans use the filter script; large plans use the concat demuxer. Any
//! silent-speed retime forces the filter script because atempo needs filters.

use std::ffi::OsStr;
use std::fmt::Write;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::NamedTempFile;
use thiserror::Error;

use crate::analysis::{Analysis, Edit};
use crate::export::{concat_path, concat_seconds};
use crate::ffprobe::{ProbeError, probe_file};
use crate::rational::Rational;
use crate::tool;

/// Default segment count up to which a single filter graph is used.
pub const DEFAULT_MAX_FILTER_SEGMENTS: usize = 12;

/// The ffmpeg rendering approach chosen for a plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderStrategy {
    /// One `filter_complex` graph with `trim`/`atrim` and `concat`.
    FilterScript,
    /// A concat-demuxer list with `inpoint`/`outpoint` directives.
    ConcatDemuxer,
}

impl std::fmt::Display for RenderStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FilterScript => f.write_str("filter-script"),
            Self::ConcatDemuxer => f.write_str("concat-demuxer"),
        }
    }
}

/// Errors produced while rendering.
#[derive(Debug, Error)]
pub enum RenderError {
    /// The input file does not exist.
    #[error("input file not found: `{0}`")]
    InputNotFound(PathBuf),
    /// ffmpeg could not be found on `PATH`.
    #[error("{}", tool::missing_message(tool))]
    ToolMissing {
        /// Name of the missing executable.
        tool: String,
    },
    /// ffprobe failed to read the input metadata.
    #[error(transparent)]
    Probe(#[from] ProbeError),
    /// A temporary concat list could not be created.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// ffmpeg could not be started.
    #[error("failed to spawn `{tool}`: {source}")]
    Spawn {
        /// Name of the executable.
        tool: String,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// ffmpeg exited with a failure status.
    #[error("`{tool}` failed with {status}: {stderr}")]
    ProcessFailed {
        /// Name of the executable.
        tool: String,
        /// Exit status reported by the OS.
        status: String,
        /// Captured standard error.
        stderr: String,
    },
    /// The plan retained no media, so there is nothing to render.
    #[error("edit plan retains no media; refusing to render an empty file")]
    NoKeptSegments,
    /// Rendering options were invalid.
    #[error("invalid render options: {0}")]
    InvalidOptions(String),
}

/// Controls codec selection and strategy choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderOptions {
    /// Stream-copy the input codecs (imprecise because cuts snap to keyframes).
    pub copy: bool,
    /// Video encoder override, e.g. `h264_nvenc` or `h264_qsv`.
    pub encoder: Option<String>,
    /// Constant rate factor used with the default `libx264` encoder.
    pub crf: u8,
    /// AAC bitrate in kilobits per second.
    pub audio_bitrate_kbps: u16,
    /// Maximum number of filter-graph segments before switching to concat.
    pub max_filter_segments: usize,
    /// Whether the source has a video stream; probed automatically by [`render`].
    pub has_video: bool,
    /// Whether the source has an audio stream; probed automatically by [`render`].
    pub has_audio: bool,
    /// Source frame rate, used to pick a retimed video rate for speed-ups.
    pub frame_rate: Option<crate::rational::FrameRate>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            copy: false,
            encoder: None,
            crf: 18,
            audio_bitrate_kbps: 192,
            max_filter_segments: DEFAULT_MAX_FILTER_SEGMENTS,
            has_video: true,
            has_audio: true,
            frame_rate: None,
        }
    }
}

impl RenderOptions {
    /// Validates numeric parameters.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::InvalidOptions`] for an out-of-range CRF or an
    /// empty audio bitrate.
    pub fn validate(&self) -> Result<(), RenderError> {
        if self.crf > 51 {
            return Err(RenderError::InvalidOptions(
                "CRF must be between 0 and 51".to_owned(),
            ));
        }
        if self.audio_bitrate_kbps == 0 {
            return Err(RenderError::InvalidOptions(
                "audio bitrate must be positive".to_owned(),
            ));
        }
        if let Some(encoder) = &self.encoder {
            if encoder.trim().is_empty() {
                return Err(RenderError::InvalidOptions(
                    "encoder name must not be empty".to_owned(),
                ));
            }
        }
        if !self.has_video && !self.has_audio {
            return Err(RenderError::InvalidOptions(
                "source has neither audio nor video streams".to_owned(),
            ));
        }
        Ok(())
    }
}

/// One retained source span and its optional playback speed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSegment {
    /// In point in the source timeline.
    pub start: Rational,
    /// Out point in the source timeline.
    pub end: Rational,
    /// Playback multiplier when this span is sped up.
    pub speed: Option<f64>,
}

/// A validated plan ready to be turned into ffmpeg arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderPlan {
    /// Retained segments in output order.
    pub segments: Vec<RenderSegment>,
    /// Strategy ffmpeg will use.
    pub strategy: RenderStrategy,
}

impl RenderPlan {
    /// Builds a render plan from an analysis.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::NoKeptSegments`] when nothing would be rendered,
    /// or [`RenderError::InvalidOptions`] for invalid options.
    pub fn from_analysis(
        analysis: &Analysis,
        options: &RenderOptions,
    ) -> Result<Self, RenderError> {
        options.validate()?;
        let segments: Vec<RenderSegment> = analysis
            .plan
            .edits
            .iter()
            .filter(|edit| edit.is_kept())
            .map(|edit| RenderSegment {
                start: edit.span().start,
                end: edit.span().end,
                speed: match edit {
                    Edit::SpeedUp { speed, .. } => Some(*speed),
                    _ => None,
                },
            })
            .collect();
        if segments.is_empty() {
            return Err(RenderError::NoKeptSegments);
        }
        let has_speedups = segments.iter().any(|segment| segment.speed.is_some());
        Ok(Self {
            strategy: choose_strategy(
                segments.len(),
                has_speedups,
                options.copy,
                options.max_filter_segments,
            ),
            segments,
        })
    }
}

/// Chooses the rendering strategy.
///
/// Stream copy requires the concat demuxer; atempo retimes require a filter
/// graph; otherwise small plans use filters and large plans use concat.
#[must_use]
pub const fn choose_strategy(
    segment_count: usize,
    has_speedups: bool,
    copy: bool,
    max_filter_segments: usize,
) -> RenderStrategy {
    if copy {
        RenderStrategy::ConcatDemuxer
    } else if has_speedups || segment_count <= max_filter_segments {
        RenderStrategy::FilterScript
    } else {
        RenderStrategy::ConcatDemuxer
    }
}

/// The result of a successful render.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOutcome {
    /// Strategy that was used.
    pub strategy: RenderStrategy,
    /// Non-fatal warnings that should be shown to the user.
    pub warnings: Vec<&'static str>,
}

/// Builds the `filter_complex` graph for `plan`.
///
/// # Panics
///
/// Panics if the plan has no segments; [`RenderPlan`] guarantees at least one.
#[must_use]
pub fn filter_script(plan: &RenderPlan, options: &RenderOptions) -> String {
    let mut chains = Vec::with_capacity(plan.segments.len() * 2);
    for (index, segment) in plan.segments.iter().enumerate() {
        let start = segment.start.to_f64();
        let end = segment.end.to_f64();
        if options.has_video {
            let pts = match segment.speed {
                Some(speed) => format!("settb=1/1000000,setpts=(PTS-STARTPTS)/{speed:.6}"),
                None => "settb=1/1000000,setpts=PTS-STARTPTS".to_owned(),
            };
            let mut chain = format!("[0:v]trim=start={start:.6}:end={end:.6},{pts}");
            if let (Some(speed), Some(rate)) = (segment.speed, options.frame_rate) {
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "frame rates are small rational numbers"
                )]
                let target = (rate.as_rational().to_f64() * speed).clamp(1.0, 1000.0);
                let _ = write!(chain, ",fps={target:.6}");
            }
            let _ = write!(chain, "[v{index}]");
            chains.push(chain);
        }
        if options.has_audio {
            let mut chain =
                format!("[0:a]atrim=start={start:.6}:end={end:.6},asetpts=PTS-STARTPTS");
            if let Some(speed) = segment.speed {
                for factor in crate::analysis::atempo_chain(speed) {
                    let _ = write!(chain, ",atempo={factor:.6}");
                }
            }
            let _ = write!(chain, "[a{index}]");
            chains.push(chain);
        }
    }
    let labels: Vec<String> = (0..plan.segments.len())
        .flat_map(|index| {
            let video = options.has_video.then(|| format!("[v{index}]"));
            let audio = options.has_audio.then(|| format!("[a{index}]"));
            [video, audio].into_iter().flatten()
        })
        .collect();
    let count = plan.segments.len();
    let outputs = if options.has_video && options.has_audio {
        "[outv][outa]"
    } else if options.has_video {
        "[outv]"
    } else {
        "[outa]"
    };
    format!(
        "{};{}concat=n={count}:v={}:a={}{outputs}",
        chains.join(";"),
        labels.join(""),
        u8::from(options.has_video),
        u8::from(options.has_audio)
    )
}

/// Builds a concat-demuxer list for `plan`.
#[must_use]
pub fn concat_list(plan: &RenderPlan, input: &Path) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    let file = concat_path(input);
    for segment in &plan.segments {
        let _ = writeln!(out, "file '{file}'");
        let _ = writeln!(out, "inpoint {}", concat_seconds(segment.start));
        let _ = writeln!(out, "outpoint {}", concat_seconds(segment.end));
        if let Some(speed) = segment.speed {
            let _ = writeln!(out, "# jumpcut speed: {speed:.6}x");
        }
    }
    out
}

/// Builds the complete ffmpeg argument vector.
#[must_use]
pub fn ffmpeg_args(
    plan: &RenderPlan,
    input: &Path,
    output: &Path,
    options: &RenderOptions,
    list_path: Option<&Path>,
) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".to_owned(),
        "-loglevel".to_owned(),
        "error".to_owned(),
        "-nostdin".to_owned(),
        "-y".to_owned(),
    ];
    match plan.strategy {
        RenderStrategy::FilterScript => {
            args.push("-i".to_owned());
            args.push(input.to_string_lossy().into_owned());
            args.push("-filter_complex".to_owned());
            args.push(filter_script(plan, options));
            if options.has_video {
                args.push("-map".to_owned());
                args.push("[outv]".to_owned());
            }
            if options.has_audio {
                args.push("-map".to_owned());
                args.push("[outa]".to_owned());
            }
            if plan.segments.iter().any(|segment| segment.speed.is_some()) {
                // Retimed video no longer lands on the source frame grid; a
                // coarse muxer timebase would round the new timestamps to the
                // original frame interval and stretch the segment.
                if is_mp4_container(output) {
                    args.push("-video_track_timescale".to_owned());
                    args.push("1000000".to_owned());
                }
                args.push("-fps_mode".to_owned());
                args.push("passthrough".to_owned());
            }
        }
        RenderStrategy::ConcatDemuxer => {
            args.push("-f".to_owned());
            args.push("concat".to_owned());
            args.push("-safe".to_owned());
            args.push("0".to_owned());
            args.push("-i".to_owned());
            args.push(list_path.unwrap_or(input).to_string_lossy().into_owned());
        }
    }

    if options.copy {
        args.push("-c:v".to_owned());
        args.push("copy".to_owned());
        if options.has_audio {
            args.push("-c:a".to_owned());
            args.push("copy".to_owned());
        }
        args.push("-avoid_negative_ts".to_owned());
        args.push("make_zero".to_owned());
    } else {
        if options.has_video {
            let encoder = options.encoder.as_deref().unwrap_or("libx264");
            args.push("-c:v".to_owned());
            args.push(encoder.to_owned());
            if options.encoder.is_none() {
                args.push("-crf".to_owned());
                args.push(options.crf.to_string());
                args.push("-preset".to_owned());
                args.push("medium".to_owned());
            }
        }
        if options.has_audio {
            args.push("-c:a".to_owned());
            args.push("aac".to_owned());
            args.push("-b:a".to_owned());
            args.push(format!("{}k", options.audio_bitrate_kbps));
        }
    }
    if !options.has_audio {
        args.push("-an".to_owned());
    }
    if !options.has_video {
        args.push("-vn".to_owned());
    }
    if is_mp4_container(output) {
        args.push("-movflags".to_owned());
        args.push("+faststart".to_owned());
    }
    args.push(output.to_string_lossy().into_owned());
    args
}

fn is_mp4_container(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(OsStr::to_str)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp4" | "m4v" | "mov")
    )
}

/// Renders `analysis` from `input` to `output`.
///
/// Non-fatal advice (keyframe imprecision, silent-speed forcing a re-encode) is
/// returned in [`RenderOutcome::warnings`] rather than logged here.
///
/// # Errors
///
/// Returns a clear error when the input is missing, ffmpeg/ffprobe are absent,
/// the plan is empty, or ffmpeg exits unsuccessfully.
pub fn render(
    input: &Path,
    output: &Path,
    analysis: &Analysis,
    options: &RenderOptions,
) -> Result<RenderOutcome, RenderError> {
    if !input.is_file() {
        return Err(RenderError::InputNotFound(input.to_path_buf()));
    }
    let ffmpeg = tool::find("ffmpeg").ok_or_else(|| RenderError::ToolMissing {
        tool: "ffmpeg".to_owned(),
    })?;
    let info = probe_file(input)?;

    let mut options = options.clone();
    options.has_video = info.video.is_some();
    options.has_audio = !info.audio.is_empty();
    options.frame_rate = info.video.as_ref().and_then(|video| video.frame_rate);
    let mut warnings = Vec::new();
    let has_speedups = analysis
        .plan
        .edits
        .iter()
        .any(|edit| matches!(edit, Edit::SpeedUp { .. }));
    if has_speedups && options.copy {
        warnings
            .push("--copy cannot apply silent-speed (atempo needs filters); re-encoding instead");
        options.copy = false;
    }
    let plan = RenderPlan::from_analysis(analysis, &options)?;
    if options.copy {
        warnings.push(
            "--copy keeps the original codecs but cuts are imprecise: ffmpeg can only start on keyframes",
        );
    }

    let mut list_file: Option<NamedTempFile> = None;
    let mut temp_dir: Option<tempfile::TempDir> = None;
    let args = if plan.strategy == RenderStrategy::ConcatDemuxer && !options.copy {
        // Re-encoded concat: render each span accurately, then join the
        // finished segments with a stream copy. This keeps the command line
        // short while avoiding the extra frames that inpoint/outpoint produce
        // on sources with reordered (B-frame) GOPs.
        let directory = tempfile::tempdir()?;
        let mut paths = Vec::with_capacity(plan.segments.len());
        for (index, segment) in plan.segments.iter().enumerate() {
            let path = directory.path().join(format!("segment-{index:05}.mp4"));
            let single = RenderPlan {
                segments: vec![*segment],
                strategy: RenderStrategy::FilterScript,
            };
            let mut segment_args = ffmpeg_args(&single, input, &path, &options, None);
            let insert_at = segment_args.len() - 1;
            segment_args.insert(insert_at, "0".to_owned());
            segment_args.insert(insert_at, "-bf".to_owned());
            run_ffmpeg(&ffmpeg, &segment_args)?;
            paths.push(path);
        }
        let mut file = NamedTempFile::new()?;
        file.write_all(segment_concat_list(&paths).as_bytes())?;
        file.flush()?;
        list_file = Some(file);
        let copy_options = RenderOptions {
            copy: true,
            ..options.clone()
        };
        temp_dir = Some(directory);
        ffmpeg_args(
            &plan,
            input,
            output,
            &copy_options,
            list_file.as_ref().map(NamedTempFile::path),
        )
    } else {
        if plan.strategy == RenderStrategy::ConcatDemuxer {
            let mut file = NamedTempFile::new()?;
            file.write_all(concat_list(&plan, input).as_bytes())?;
            file.flush()?;
            list_file = Some(file);
        }
        ffmpeg_args(
            &plan,
            input,
            output,
            &options,
            list_file.as_ref().map(NamedTempFile::path),
        )
    };
    run_ffmpeg(&ffmpeg, &args)?;
    drop(temp_dir);
    Ok(RenderOutcome {
        strategy: plan.strategy,
        warnings,
    })
}

fn run_ffmpeg(ffmpeg: &Path, args: &[String]) -> Result<(), RenderError> {
    let result = Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| RenderError::Spawn {
            tool: "ffmpeg".to_owned(),
            source,
        })?;
    if result.status.success() {
        Ok(())
    } else {
        Err(RenderError::ProcessFailed {
            tool: "ffmpeg".to_owned(),
            status: result.status.to_string(),
            stderr: String::from_utf8_lossy(&result.stderr).trim().to_owned(),
        })
    }
}

/// Builds a concat list that joins already-trimmed segment files.
#[must_use]
pub fn segment_concat_list(paths: &[PathBuf]) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    for path in paths {
        let _ = writeln!(out, "file '{}'", concat_path(path));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{
        AnalysisConfig, FrameConfig, Margins, StreamSelection, ThresholdSelection, analyze_file,
        analyze_samples,
    };
    use crate::rational::FrameRate;
    use crate::{DecodeOptions, Rational};
    use std::process::Command;

    fn config(speed: Option<f64>) -> AnalysisConfig {
        AnalysisConfig {
            frame: FrameConfig::default(),
            threshold: ThresholdSelection::Absolute(-35.0),
            hysteresis_db: 0.0,
            min_silence: Rational::new(1, 2).unwrap(),
            min_clip: Rational::new(1, 10).unwrap(),
            margins: Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            merge_gap: Rational::ZERO,
            silent_speed: speed,
            frame_rate: Some(FrameRate::new(10, 1).unwrap()),
        }
    }

    /// 1 s speech, 1 s silence, 1 s speech, 1 s silence at 16 kHz.
    fn samples() -> Vec<f32> {
        let mut values = Vec::with_capacity(64_000);
        for &(count, amplitude) in &[
            (16_000, 0.5_f32),
            (16_000, 0.0),
            (16_000, 0.5),
            (16_000, 0.0),
        ] {
            values.resize(values.len() + count, amplitude);
        }
        values
    }

    fn plan(analysis: &Analysis, options: &RenderOptions) -> RenderPlan {
        RenderPlan::from_analysis(analysis, options).unwrap()
    }

    #[test]
    fn strategy_depends_on_segment_count_copy_and_speed() {
        assert_eq!(
            choose_strategy(2, false, false, 12),
            RenderStrategy::FilterScript
        );
        assert_eq!(
            choose_strategy(13, false, false, 12),
            RenderStrategy::ConcatDemuxer
        );
        assert_eq!(
            choose_strategy(1, true, false, 0),
            RenderStrategy::FilterScript
        );
        assert_eq!(
            choose_strategy(1, false, true, 12),
            RenderStrategy::ConcatDemuxer
        );
    }

    #[test]
    fn filter_script_trims_concats_and_chains_atempo() {
        let analysis = analyze_samples(&samples(), 16_000, &config(Some(4.0))).unwrap();
        let options = RenderOptions::default();
        let script = filter_script(&plan(&analysis, &options), &options);
        assert!(
            script.contains("trim=start=0.000000:end=1.000000,settb=1/1000000,setpts=PTS-STARTPTS")
        );
        assert!(script.contains("atrim=start=1.000000:end=2.000000"));
        assert!(script.contains("atempo=4.000000"));
        assert!(script.contains("settb=1/1000000,setpts=(PTS-STARTPTS)/4.000000"));
        assert!(script.contains("concat=n=4:v=1:a=1[outv][outa]"));
    }

    #[test]
    fn audio_only_and_video_only_graphs_use_the_right_concat_mode() {
        let analysis = analyze_samples(&samples(), 16_000, &config(None)).unwrap();
        let audio_only = RenderOptions {
            has_video: false,
            ..RenderOptions::default()
        };
        assert!(
            filter_script(&plan(&analysis, &audio_only), &audio_only)
                .contains("concat=n=2:v=0:a=1[outa]")
        );
        let video_only = RenderOptions {
            has_audio: false,
            ..RenderOptions::default()
        };
        assert!(
            filter_script(&plan(&analysis, &video_only), &video_only)
                .contains("concat=n=2:v=1:a=0[outv]")
        );
    }

    #[test]
    fn ffmpeg_args_reflect_default_hardware_and_copy_encoders() {
        let analysis = analyze_samples(&samples(), 16_000, &config(None)).unwrap();
        let input = Path::new("/media/in.mp4");
        let output = Path::new("/media/out.mp4");
        let default_options = RenderOptions::default();
        let default_plan = plan(&analysis, &default_options);
        let args = ffmpeg_args(&default_plan, input, output, &default_options, None);
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-crf", "18"]));
        assert!(args.windows(2).any(|w| w == ["-c:a", "aac"]));
        assert!(args.windows(2).any(|w| w == ["-b:a", "192k"]));
        assert!(args.contains(&"-filter_complex".to_owned()));

        let hw = RenderOptions {
            encoder: Some("h264_nvenc".to_owned()),
            ..RenderOptions::default()
        };
        let hw_args = ffmpeg_args(&plan(&analysis, &hw), input, output, &hw, None);
        assert!(hw_args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert!(!hw_args.contains(&"-crf".to_owned()));

        let copy = RenderOptions {
            copy: true,
            ..RenderOptions::default()
        };
        let copy_plan = plan(&analysis, &copy);
        let list = Path::new("/tmp/list.txt");
        let copy_args = ffmpeg_args(&copy_plan, input, output, &copy, Some(list));
        assert!(copy_args.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(copy_args.windows(2).any(|w| w == ["-c:a", "copy"]));
        assert!(copy_args.windows(2).any(|w| w == ["-safe", "0"]));
        assert!(copy_args.contains(&list.to_string_lossy().into_owned()));
    }

    #[test]
    fn concat_list_has_one_in_out_pair_per_segment() {
        let analysis = analyze_samples(&samples(), 16_000, &config(None)).unwrap();
        let list = concat_list(
            &plan(&analysis, &RenderOptions::default()),
            Path::new("/media/in.mp4"),
        );
        assert!(list.starts_with("ffconcat version 1.0\n"));
        assert_eq!(list.matches("file ").count(), 2);
        assert!(list.contains("inpoint 2.000000\noutpoint 3.000000\n"));
    }

    #[test]
    fn segment_concat_list_references_trimmed_files() {
        let list = segment_concat_list(&[
            PathBuf::from("/tmp/segment-00000.mp4"),
            PathBuf::from("/tmp/segment-00001.mp4"),
        ]);
        assert_eq!(
            list,
            "ffconcat version 1.0\nfile '/tmp/segment-00000.mp4'\nfile '/tmp/segment-00001.mp4'\n"
        );
    }

    #[test]
    fn invalid_render_options_are_rejected() {
        assert!(
            RenderOptions {
                crf: 52,
                ..RenderOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RenderOptions {
                audio_bitrate_kbps: 0,
                ..RenderOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RenderOptions {
                encoder: Some(String::new()),
                ..RenderOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RenderOptions {
                has_video: false,
                has_audio: false,
                ..RenderOptions::default()
            }
            .validate()
            .is_err()
        );
        assert!(RenderOptions::default().validate().is_ok());
    }

    #[test]
    fn empty_plan_is_rejected() {
        let analysis = analyze_samples(&vec![0.0_f32; 16_000], 16_000, &config(None)).unwrap();
        assert!(matches!(
            RenderPlan::from_analysis(&analysis, &RenderOptions::default()),
            Err(RenderError::NoKeptSegments)
        ));
    }

    fn generate_media(path: &Path) -> bool {
        if !tool::is_available("ffmpeg") || !tool::is_available("ffprobe") {
            eprintln!("skipping end-to-end render: ffmpeg/ffprobe not installed");
            return false;
        }
        let graph = "[1:a]volume='if(between(t,1,2)+between(t,3,4),0,1)':eval=frame[a]";
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=10:duration=4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=4",
                "-filter_complex",
                graph,
                "-map",
                "0:v:0",
                "-map",
                "[a]",
                "-c:v",
                "libx264",
                "-g",
                "10",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg could not generate the test video");
        true
    }

    fn duration(path: &Path) -> f64 {
        probe_file(path)
            .unwrap()
            .duration_seconds()
            .expect("output duration was not reported")
    }

    #[test]
    fn end_to_end_filter_render_duration_matches_plan() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("source.mp4");
        let output = directory.path().join("filter.mp4");
        if !generate_media(&input) {
            return;
        }
        let analysis = analyze_file(
            &input,
            StreamSelection::Single(0),
            DecodeOptions::default(),
            config(None),
        )
        .unwrap();
        assert_eq!(analysis.plan.kept_segments().len(), 2);
        assert_eq!(analysis.output_duration(), Rational::new(2, 1).unwrap());
        let outcome = render(&input, &output, &analysis, &RenderOptions::default()).unwrap();
        assert_eq!(outcome.strategy, RenderStrategy::FilterScript);
        assert!(
            (duration(&output) - 2.0).abs() <= 0.1,
            "duration {}",
            duration(&output)
        );
    }

    #[test]
    fn end_to_end_concat_render_duration_matches_plan() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("source.mp4");
        let output = directory.path().join("concat.mp4");
        if !generate_media(&input) {
            return;
        }
        let analysis = analyze_file(
            &input,
            StreamSelection::Single(0),
            DecodeOptions::default(),
            config(None),
        )
        .unwrap();
        let options = RenderOptions {
            max_filter_segments: 0,
            ..RenderOptions::default()
        };
        let outcome = render(&input, &output, &analysis, &options).unwrap();
        assert_eq!(outcome.strategy, RenderStrategy::ConcatDemuxer);
        assert!(
            (duration(&output) - 2.0).abs() <= 0.1,
            "duration {}",
            duration(&output)
        );
    }

    #[test]
    fn end_to_end_silent_speed_uses_atempo_and_shortens_output() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("source.mp4");
        let output = directory.path().join("speed.mp4");
        if !generate_media(&input) {
            return;
        }
        let analysis = analyze_file(
            &input,
            StreamSelection::Single(0),
            DecodeOptions::default(),
            config(Some(4.0)),
        )
        .unwrap();
        assert_eq!(analysis.output_duration(), Rational::new(5, 2).unwrap());
        render(&input, &output, &analysis, &RenderOptions::default()).unwrap();
        assert!(
            (duration(&output) - 2.5).abs() <= 0.1,
            "duration {}",
            duration(&output)
        );
    }

    #[test]
    fn end_to_end_copy_keeps_codecs_and_warns() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("source.mp4");
        let output = directory.path().join("copy.mp4");
        if !generate_media(&input) {
            return;
        }
        let analysis = analyze_file(
            &input,
            StreamSelection::Single(0),
            DecodeOptions::default(),
            config(None),
        )
        .unwrap();
        let outcome = render(
            &input,
            &output,
            &analysis,
            &RenderOptions {
                copy: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.strategy, RenderStrategy::ConcatDemuxer);
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("keyframe"))
        );
        let info = probe_file(&output).unwrap();
        assert_eq!(info.video.unwrap().codec, "h264");
        assert_eq!(info.audio[0].codec, "aac");
    }
}

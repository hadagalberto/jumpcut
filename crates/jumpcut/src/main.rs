//! Command-line entry point for jumpcut.

#![warn(clippy::pedantic)]
#![deny(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jumpcut_core::{
    AnalysisConfig, Dbfs, DecodeOptions, ExportContext, ExportFormat, FrameRate, RenderOptions,
    Seconds, StreamSelection, ThresholdSelection, Timecode, VideoInfo, analyze_file,
    compact_analysis, export, parse_margins, preview, probe_file, render,
};

#[derive(Debug, Parser)]
#[command(
    name = "jumpcut",
    version,
    about = "Remove silences from videos and podcasts, fast",
    long_about = "Single-binary CLI for silence removal, ffmpeg rendering and NLE timeline export.",
    subcommand_precedence_over_arg = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Options for the top-level `jumpcut INPUT -o OUTPUT` form.
    #[command(flatten)]
    render: RenderArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read media metadata with ffprobe.
    Probe {
        /// Input media file.
        input: PathBuf,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Format an elapsed frame count as SMPTE timecode.
    Timecode {
        /// Number of elapsed frames.
        frames: u64,
        /// Frame rate, e.g. 25, 29.97 or 30000/1001.
        #[arg(long, default_value = "29.97")]
        rate: FrameRate,
        /// Use drop-frame counting (29.97 / 59.94 only).
        #[arg(long)]
        drop_frame: bool,
    },
    /// Analyse audio and preview silence removal without rendering.
    Analyze(Box<AnalyzeArgs>),
    /// Render or export an edit timeline (also available as the top-level form).
    Render(Box<RenderArgs>),
}

/// Analysis knobs shared by the `analyze` and `render` commands.
#[derive(Debug, clap::Args)]
struct AnalyzeOptions {
    /// Measurement window length (default 30 ms).
    #[arg(long)]
    window: Option<Seconds>,
    /// Distance between frames (default 10 ms).
    #[arg(long)]
    hop: Option<Seconds>,
    /// Absolute silence threshold in dBFS, e.g. -35dB.
    #[arg(long, value_name = "dB", conflicts_with = "auto")]
    threshold: Option<Dbfs>,
    /// Adapt the threshold from the loudness distribution.
    #[arg(long)]
    auto: bool,
    /// Hysteresis width in dB (default 3).
    #[arg(long, default_value_t = 3.0)]
    hysteresis: f64,
    /// Shortest silence worth removing (default 0.5s).
    #[arg(long, default_value = "0.5s")]
    min_silence: Seconds,
    /// Shortest kept segment (default 0.3s).
    #[arg(long, default_value = "0.3s")]
    min_clip: Seconds,
    /// Margin before and after speech, or `before,after`.
    #[arg(long, default_value = "0.1s", value_name = "SECONDS")]
    margin: String,
    /// Merge kept gaps shorter than this into the surrounding cut.
    #[arg(long, default_value = "0s")]
    merge_gap: Seconds,
    /// Speed up silences by this factor instead of cutting them.
    #[arg(long, value_name = "FACTOR")]
    silent_speed: Option<f64>,
    /// Analyse one audio-relative stream index (default: first).
    #[arg(long, conflicts_with = "combine_streams")]
    audio_stream: Option<u32>,
    /// Combine all audio streams by maximum loudness.
    #[arg(long)]
    combine_streams: bool,
    /// Override the frame rate used for exact boundary snapping.
    #[arg(long, value_name = "RATE")]
    fps: Option<FrameRate>,
}

/// Arguments for the `analyze` subcommand, boxed to keep [`Command`] small.
#[derive(Debug, clap::Args)]
struct AnalyzeArgs {
    /// Input media file.
    input: PathBuf,
    #[command(flatten)]
    options: AnalyzeOptions,
    /// Emit machine-readable analysis JSON instead of a terminal preview.
    #[arg(long)]
    json: bool,
    /// Emit only the edit summary with --json (no per-frame measurements).
    #[arg(long)]
    compact: bool,
}

/// Arguments for rendering and exporting, also flattened at the top level.
#[derive(Debug, clap::Args)]
struct RenderArgs {
    /// Input media file (top-level render/export form).
    input: Option<PathBuf>,
    /// Rendered media file, or destination for `--export`.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,
    #[command(flatten)]
    options: AnalyzeOptions,
    /// Write an edit timeline instead of rendering.
    #[arg(long, value_name = "FORMAT")]
    export: Option<ExportFormat>,
    /// Print a terminal summary (segments, removed time, sparkline).
    #[arg(long)]
    preview: bool,
    /// Stream-copy codecs; cuts may snap to the nearest keyframe (warns).
    #[arg(long)]
    copy: bool,
    /// Video encoder passed through to ffmpeg, e.g. `h264_nvenc`.
    #[arg(long, value_name = "ENCODER")]
    hw_encoder: Option<String>,
    /// Force drop-frame timecodes in exported timelines.
    #[arg(long)]
    drop_frame: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Probe { input, json }) => {
            let info = probe_file(&input)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                print_probe_report(&info);
            }
        }
        Some(Command::Timecode {
            frames,
            rate,
            drop_frame,
        }) => {
            let timecode = Timecode::new(frames, rate, drop_frame)?;
            println!("{}", timecode.format());
        }
        Some(Command::Analyze(args)) => run_analyze(*args)?,
        Some(Command::Render(args)) => run_render(*args)?,
        None => run_render(cli.render)?,
    }
    Ok(())
}

fn settings(options: &AnalyzeOptions) -> Result<(AnalysisConfig, StreamSelection)> {
    let mut frame = jumpcut_core::FrameConfig::default();
    if let Some(window) = options.window {
        frame.window = window;
    }
    if let Some(hop) = options.hop {
        frame.hop = hop;
    }
    let config = AnalysisConfig {
        frame,
        threshold: if options.auto {
            ThresholdSelection::Auto
        } else {
            ThresholdSelection::Absolute(options.threshold.map_or(-35.0, |value| value.0))
        },
        hysteresis_db: options.hysteresis,
        min_silence: options.min_silence.0,
        min_clip: options.min_clip.0,
        margins: parse_margins(&options.margin)?,
        merge_gap: options.merge_gap.0,
        silent_speed: options.silent_speed,
        frame_rate: options.fps,
    };
    config.validate()?;
    let selection = if options.combine_streams {
        StreamSelection::CombineMax
    } else {
        StreamSelection::Single(options.audio_stream.unwrap_or(0))
    };
    Ok((config, selection))
}

fn run_analyze(args: AnalyzeArgs) -> Result<()> {
    let AnalyzeArgs {
        input,
        options,
        json,
        compact,
    } = args;
    let (config, selection) = settings(&options)?;
    let analysis = analyze_file(&input, selection, DecodeOptions::default(), config)
        .with_context(|| format!("failed to analyse {}", input.display()))?;
    if json {
        let value = if compact {
            compact_analysis(&analysis)
        } else {
            serde_json::to_value(&analysis)?
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        if compact {
            eprintln!("warning: --compact only affects --json output");
        }
        print!("{}", preview(&analysis));
    }
    Ok(())
}

fn run_render(args: RenderArgs) -> Result<()> {
    let RenderArgs {
        input,
        output,
        options,
        export: format,
        preview: show_preview,
        copy,
        hw_encoder,
        drop_frame,
    } = args;
    let input = input.context("missing input file; usage: jumpcut INPUT -o OUTPUT")?;
    let (config, selection) = settings(&options)?;
    let analysis = analyze_file(&input, selection, DecodeOptions::default(), config.clone())
        .with_context(|| format!("failed to analyse {}", input.display()))?;

    if show_preview {
        print!("{}", preview(&analysis));
    }

    if let Some(format) = format {
        let media = probe_file(&input)?;
        let frame_rate = config
            .frame_rate
            .or(analysis.plan.frame_rate)
            .or_else(|| media.video.as_ref().and_then(|video| video.frame_rate))
            .unwrap_or(FrameRate::FPS_30);
        let use_drop_frame = drop_frame || frame_rate.drop_frames_per_minute().is_some();
        let has_video = media.video.is_some();
        let video = media
            .video
            .as_ref()
            .map(VideoInfo::from_stream)
            .unwrap_or_default();
        let context = ExportContext::with_timecode(&input, &analysis, frame_rate, use_drop_frame)
            .with_video(video)
            .with_has_video(has_video);
        let text = export(&context, format)?;
        match output {
            Some(path) => {
                std::fs::write(&path, text)
                    .with_context(|| format!("failed to write {}", path.display()))?;
                println!("wrote {format} timeline to {}", path.display());
            }
            None => print!("{text}"),
        }
        return Ok(());
    }

    if show_preview && output.is_none() {
        return Ok(());
    }
    let output = output.context("output file required (-o OUTPUT) when rendering")?;
    let render_options = RenderOptions {
        copy,
        encoder: hw_encoder,
        ..RenderOptions::default()
    };
    let outcome = render(&input, &output, &analysis, &render_options)?;
    for warning in outcome.warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "rendered {} -> {} ({} strategy, {:.3} s)",
        input.display(),
        output.display(),
        outcome.strategy,
        analysis.output_duration().to_f64()
    );
    Ok(())
}

fn print_probe_report(info: &jumpcut_core::MediaInfo) {
    println!("file:     {}", info.path.display());
    println!(
        "duration: {}",
        info.duration_seconds()
            .map_or_else(|| "unknown".to_owned(), |seconds| format!("{seconds:.3} s"))
    );
    if let Some(video) = &info.video {
        println!(
            "video:    {} {}x{} @ {}",
            video.codec,
            video.width,
            video.height,
            video
                .frame_rate
                .map_or_else(|| "unknown fps".to_owned(), |rate| format!("{rate} fps"))
        );
    }
    for audio in &info.audio {
        println!(
            "audio:    stream {} {} {} Hz, {} ch",
            audio.index, audio.codec, audio.sample_rate, audio.channels
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_flag_is_parsed_for_json_analysis() {
        let cli = Cli::try_parse_from(["jumpcut", "analyze", "episode.wav", "--json", "--compact"])
            .unwrap();
        match cli.command {
            Some(Command::Analyze(args)) => {
                assert!(args.json);
                assert!(args.compact);
            }
            other => panic!("expected analyze command, got {other:?}"),
        }
    }
}

//! Command-line entry point for jumpcut.

#![warn(clippy::pedantic)]
#![deny(unsafe_code)]

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use jumpcut_core::{
    Analysis, AnalysisConfig, Dbfs, DecodeOptions, FrameRate, Seconds, StreamSelection,
    ThresholdSelection, Timecode, analyze_file, parse_margins, probe_file, sparkline,
};

#[derive(Debug, Parser)]
#[command(
    name = "jumpcut",
    version,
    about = "Remove silences from videos and podcasts, fast",
    long_about = "Single-binary CLI for silence removal and NLE timeline export."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
    /// Analyse audio and plan silence removal without rendering.
    Analyze(Box<AnalyzeArgs>),
}

/// Arguments for the `analyze` subcommand, boxed to keep [`Command`] small.
#[derive(Debug, clap::Args)]
struct AnalyzeArgs {
    /// Input media file.
    input: PathBuf,
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
    /// Emit machine-readable JSON instead of a terminal preview.
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Probe { input, json } => {
            let info = probe_file(&input)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                print_probe_report(&info);
            }
        }
        Command::Timecode {
            frames,
            rate,
            drop_frame,
        } => {
            let timecode = Timecode::new(frames, rate, drop_frame)?;
            println!("{}", timecode.format());
        }
        Command::Analyze(args) => {
            let AnalyzeArgs {
                input,
                window,
                hop,
                threshold,
                auto,
                hysteresis,
                min_silence,
                min_clip,
                margin,
                merge_gap,
                silent_speed,
                audio_stream,
                combine_streams,
                fps,
                json,
            } = *args;
            let mut frame = jumpcut_core::FrameConfig::default();
            if let Some(window) = window {
                frame.window = window;
            }
            if let Some(hop) = hop {
                frame.hop = hop;
            }
            let config = AnalysisConfig {
                frame,
                threshold: if auto {
                    ThresholdSelection::Auto
                } else {
                    ThresholdSelection::Absolute(threshold.map_or(-35.0, |value| value.0))
                },
                hysteresis_db: hysteresis,
                min_silence: min_silence.0,
                min_clip: min_clip.0,
                margins: parse_margins(&margin)?,
                merge_gap: merge_gap.0,
                silent_speed,
                frame_rate: fps,
            };
            config.validate()?;
            let selection = if combine_streams {
                StreamSelection::CombineMax
            } else {
                StreamSelection::Single(audio_stream.unwrap_or(0))
            };
            let analysis = analyze_file(&input, selection, DecodeOptions::default(), config)
                .with_context(|| format!("failed to analyse {}", input.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&analysis)?);
            } else {
                print_analysis_report(&analysis);
            }
        }
    }
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

fn print_analysis_report(analysis: &Analysis) {
    let source = analysis.plan.source_duration.to_f64();
    let removed = analysis.removed_duration().to_f64();
    let output = analysis.output_duration().to_f64();
    let percent = if source > 0.0 {
        removed / source * 100.0
    } else {
        0.0
    };
    println!("source:   {source:.3} s");
    println!(
        "threshold:{:+.1} dBFS{}",
        analysis.thresholds.base_dbfs,
        if analysis.thresholds.adaptive {
            " (auto)"
        } else {
            ""
        }
    );
    println!(
        "kept:     {} segment(s)",
        analysis.plan.kept_segments().len()
    );
    println!("removed:  {removed:.3} s ({percent:.1}%)");
    println!("output:   {output:.3} s");
    println!("loudness: {}", sparkline(&analysis.frames, 64));
    for (index, edit) in analysis.plan.edits.iter().enumerate() {
        let span = edit.span();
        let start = span.start.to_f64();
        let end = span.end.to_f64();
        match edit {
            jumpcut_core::Edit::Keep { .. } => {
                println!("  {index:>3}: keep  {start:8.3} .. {end:8.3}");
            }
            jumpcut_core::Edit::Cut { .. } => {
                println!("  {index:>3}: cut   {start:8.3} .. {end:8.3}");
            }
            jumpcut_core::Edit::SpeedUp { speed, .. } => {
                println!("  {index:>3}: speed {start:8.3} .. {end:8.3} @ {speed}x");
            }
        }
    }
}

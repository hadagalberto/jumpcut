//! Command-line entry point for jumpcut (milestone 1 scaffold).

#![warn(clippy::pedantic)]
#![deny(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use jumpcut_core::{FrameRate, Timecode, probe_file};

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

//! File-level orchestration: probe, decode, analyse and plan in one call.

use std::path::Path;

use super::frame::FrameAnalyzer;
use super::{Analysis, AnalysisConfig, AnalysisError, analyze_frames};
use crate::decode::{DecodeOptions, PcmDecoder};
use crate::ffprobe::probe_file;
use crate::rational::Rational;

/// Which audio streams feed the analysis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamSelection {
    /// One audio-relative stream index, matching `--audio-stream N` / `0:a:N`.
    Single(u32),
    /// Decode every audio stream and combine them by per-frame maximum RMS.
    CombineMax,
}

fn decode_frames(
    input: &Path,
    options: DecodeOptions,
    config: &AnalysisConfig,
) -> Result<Vec<super::frame::AudioFrame>, AnalysisError> {
    let mut decoder = PcmDecoder::spawn(input, options)?;
    let mut analyzer = FrameAnalyzer::new(options.sample_rate, config.frame)?;
    let mut frames = Vec::new();
    loop {
        let chunk = decoder.read_chunk(8_192)?;
        if chunk.is_empty() {
            break;
        }
        frames.extend(analyzer.push(&chunk));
    }
    frames.extend(analyzer.finish());
    decoder.finish()?;
    Ok(frames)
}

/// Combines per-stream frames by taking the maximum RMS and peak at each hop.
///
/// All streams must share the same frame grid; otherwise
/// [`AnalysisError::StreamMismatch`] is returned.
///
/// # Errors
///
/// Returns [`AnalysisError::NoAudioStreams`] for empty input or
/// [`AnalysisError::StreamMismatch`] when frame grids differ.
pub fn combine_frames(
    streams: &[Vec<super::frame::AudioFrame>],
) -> Result<Vec<super::frame::AudioFrame>, AnalysisError> {
    let first = streams.first().ok_or(AnalysisError::NoAudioStreams)?;
    for stream in streams {
        if stream.len() != first.len() {
            return Err(AnalysisError::StreamMismatch {
                expected: first.len(),
                actual: stream.len(),
            });
        }
    }
    let mut combined = Vec::with_capacity(first.len());
    for (index, base) in first.iter().enumerate() {
        let mut frame = *base;
        for stream in &streams[1..] {
            let other = &stream[index];
            if other.start != base.start {
                return Err(AnalysisError::StreamMismatch {
                    expected: first.len(),
                    actual: stream.len(),
                });
            }
            frame.rms_dbfs = frame.rms_dbfs.max(other.rms_dbfs);
            frame.peak_dbfs = frame.peak_dbfs.max(other.peak_dbfs);
        }
        combined.push(frame);
    }
    Ok(combined)
}

/// Analyses a media file end to end.
///
/// `ffprobe` supplies the source duration and frame rate used for snapping;
/// `ffmpeg` streams `f32le` PCM through [`PcmDecoder`]. No full waveform is
/// ever held in memory.
///
/// # Errors
///
/// Propagates probe/decode failures, a missing audio stream, or an invalid
/// analysis configuration.
pub fn analyze_file(
    input: &Path,
    selection: StreamSelection,
    decode_options: DecodeOptions,
    config: AnalysisConfig,
) -> Result<Analysis, AnalysisError> {
    let info = probe_file(input)?;
    let indices: Vec<u32> = match selection {
        StreamSelection::Single(index) => vec![index],
        StreamSelection::CombineMax => {
            if info.audio.is_empty() {
                return Err(AnalysisError::NoAudioStreams);
            }
            #[allow(
                clippy::cast_possible_truncation,
                reason = "audio stream counts are small"
            )]
            let count = info.audio.len() as u32;
            (0..count).collect()
        }
    };

    let mut streams = Vec::with_capacity(indices.len());
    for index in indices {
        let mut options = decode_options;
        options.audio_stream = Some(index);
        streams.push(decode_frames(input, options, &config)?);
    }
    let frames = if streams.len() == 1 {
        streams.remove(0)
    } else {
        combine_frames(&streams)?
    };

    let mut config = config;
    if config.frame_rate.is_none() {
        config.frame_rate = info.video.as_ref().and_then(|video| video.frame_rate);
    }
    let duration = info.duration.unwrap_or_else(|| {
        frames
            .last()
            .map_or(Rational::ZERO, |frame| frame.start + frame.window_duration)
    });

    analyze_frames(&frames, &config, duration, decode_options.sample_rate)
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test frame indices are small"
)]
mod tests {
    use super::*;
    use crate::analysis::frame::AudioFrame;
    use crate::rational::Rational;

    fn frame(index: i128, rms: f64) -> AudioFrame {
        AudioFrame {
            index: index as u64,
            start: Rational::new(index, 100).unwrap(),
            duration: Rational::new(1, 100).unwrap(),
            window_duration: Rational::new(3, 100).unwrap(),
            rms_dbfs: rms,
            peak_dbfs: rms,
        }
    }

    #[test]
    fn combine_frames_takes_maximum_loudness() {
        let left = vec![frame(0, -40.0), frame(1, -6.0)];
        let right = vec![frame(0, -12.0), frame(1, -60.0)];
        let combined = combine_frames(&[left, right]).unwrap();
        assert_eq!(combined.len(), 2);
        assert_eq!(combined[0].rms_dbfs, -12.0);
        assert_eq!(combined[1].rms_dbfs, -6.0);
        assert_eq!(combined[0].peak_dbfs, -12.0);
    }

    #[test]
    fn analyzes_a_real_wav_end_to_end() {
        use std::process::Command;
        if !crate::tool::is_available("ffmpeg") || !crate::tool::is_available("ffprobe") {
            eprintln!("skipping: ffmpeg/ffprobe not installed");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let wav = directory.path().join("speech-silence.wav");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=16000:duration=2",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=16000:cl=mono:d=2",
                "-filter_complex",
                "[0:a]atrim=0:1[t1];[0:a]atrim=1:2[t2];[1:a]atrim=0:1[s1];[1:a]atrim=0:1[s2];[t1][s1][t2][s2]concat=n=4:v=0:a=1[out]",
                "-map",
                "[out]",
                "-ac",
                "1",
                "-y",
            ])
            .arg(&wav)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg could not create the test WAV");

        let config = crate::AnalysisConfig {
            threshold: crate::ThresholdSelection::Absolute(-35.0),
            hysteresis_db: 0.0,
            min_silence: Rational::new(1, 2).unwrap(),
            min_clip: Rational::ZERO,
            margins: crate::Margins {
                before: Rational::ZERO,
                after: Rational::ZERO,
            },
            ..crate::AnalysisConfig::default()
        };
        let analysis = analyze_file(
            &wav,
            StreamSelection::Single(0),
            DecodeOptions::default(),
            config,
        )
        .unwrap();
        let cuts: Vec<(Rational, Rational)> = analysis
            .plan
            .cut_segments()
            .into_iter()
            .map(|span| (span.start, span.end))
            .collect();
        assert_eq!(
            cuts,
            vec![
                (Rational::new(1, 1).unwrap(), Rational::new(2, 1).unwrap()),
                (Rational::new(3, 1).unwrap(), Rational::new(4, 1).unwrap()),
            ]
        );
    }

    #[test]
    fn combine_frames_rejects_mismatched_grids() {
        let left = vec![frame(0, -6.0)];
        let right = vec![frame(0, -6.0), frame(1, -6.0)];
        assert!(matches!(
            combine_frames(&[left, right]),
            Err(AnalysisError::StreamMismatch { .. })
        ));
        assert!(matches!(
            combine_frames(&[]),
            Err(AnalysisError::NoAudioStreams)
        ));
    }
}

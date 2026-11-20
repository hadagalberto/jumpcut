//! Streaming frame-based RMS/peak loudness measurement.
//!
//! PCM samples are consumed in bounded chunks; only the samples needed for the
//! current measurement window are retained, so analysing a multi-hour file
//! never loads the whole waveform into memory. Each emitted [`AudioFrame`]
//! carries both the *measurement window* used to compute loudness and the
//! shorter *hop interval* that the frame represents in time.

use serde::Serialize;

use super::{AnalysisError, DBFS_FLOOR, Seconds};

/// Timing of the analysis frames.
///
/// The default is a 30 ms measurement window advanced by a 10 ms hop, matching
/// the milestone specification. Windows may overlap (`window > hop`), butt up
/// (`window == hop`) or leave gaps (`window < hop`); all are handled exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct FrameConfig {
    /// Length of the window over which RMS and peak are measured.
    pub window: Seconds,
    /// Distance between the start of consecutive frames.
    pub hop: Seconds,
}

impl Default for FrameConfig {
    fn default() -> Self {
        Self {
            window: Seconds(Rational::new(3, 100).expect("constant fraction")),
            hop: Seconds(Rational::new(1, 100).expect("constant fraction")),
        }
    }
}

use crate::rational::Rational;

/// One analysis frame: a loudness measurement plus its exact place in time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct AudioFrame {
    /// Zero-based frame index (`start / hop`).
    pub index: u64,
    /// Start of the frame's hop interval, in seconds.
    pub start: Rational,
    /// Length of the hop interval this frame represents, in seconds.
    ///
    /// Usually equal to the configured hop; the final frame may be shorter
    /// when the PCM ends mid-hop.
    pub duration: Rational,
    /// Length of the measurement window, in seconds.
    ///
    /// Usually equal to the configured window; trailing frames may be shorter.
    pub window_duration: Rational,
    /// RMS loudness of the measurement window in dBFS, floored at [`DBFS_FLOOR`].
    pub rms_dbfs: f64,
    /// Peak loudness of the measurement window in dBFS, floored at [`DBFS_FLOOR`].
    pub peak_dbfs: f64,
}

/// Converts a linear amplitude to decibels relative to full scale.
///
/// Amplitudes at or below zero map to [`DBFS_FLOOR`] so every frame carries a
/// finite, JSON-safe value.
#[must_use]
pub fn amplitude_to_dbfs(amplitude: f64) -> f64 {
    if amplitude > 0.0 && amplitude.is_finite() {
        20.0 * amplitude.log10()
    } else {
        DBFS_FLOOR
    }
}

/// Measures RMS and peak of a window, returning floored dBFS values.
fn measure(window: &[f32]) -> (f64, f64) {
    debug_assert!(!window.is_empty(), "measurement window must not be empty");
    let mut sum_squares = 0.0_f64;
    let mut peak = 0.0_f32;
    for &sample in window {
        let value = if sample.is_finite() { sample } else { 0.0 };
        sum_squares += f64::from(value) * f64::from(value);
        peak = peak.max(value.abs());
    }
    #[allow(clippy::cast_precision_loss, reason = "frame lengths are small")]
    let mean = sum_squares / window.len() as f64;
    let rms = amplitude_to_dbfs(mean.sqrt());
    let peak = amplitude_to_dbfs(f64::from(peak));
    (rms.max(DBFS_FLOOR), peak.max(DBFS_FLOOR))
}

fn usize_to_i128(value: usize) -> i128 {
    i128::try_from(value).unwrap_or(i128::MAX)
}

fn duration_to_samples(duration: Rational, sample_rate: u32) -> usize {
    let frames = duration * Rational::from(i64::from(sample_rate));
    usize::try_from(frames.round()).unwrap_or(usize::MAX)
}

/// Streaming converter from PCM samples to [`AudioFrame`]s.
///
/// Feed successive chunks with [`Self::push`] and call [`Self::finish`] once
/// the input is exhausted to emit the trailing (possibly partial) frames.
#[derive(Debug)]
pub struct FrameAnalyzer {
    config: FrameConfig,
    sample_rate: u32,
    window_samples: usize,
    hop_samples: usize,
    buffer: Vec<f32>,
    buffer_start: u64,
    next_hop: u64,
    total_samples: u64,
}

impl FrameAnalyzer {
    /// Creates an analyzer for `sample_rate` Hz.
    ///
    /// # Errors
    ///
    /// Returns [`AnalysisError::InvalidConfig`] when the sample rate is zero or
    /// the frame window/hop rounds to zero samples.
    pub fn new(sample_rate: u32, config: FrameConfig) -> Result<Self, AnalysisError> {
        if sample_rate == 0 {
            return Err(AnalysisError::InvalidConfig(
                "sample rate must be positive".to_owned(),
            ));
        }
        let window_samples = duration_to_samples(config.window.0, sample_rate);
        let hop_samples = duration_to_samples(config.hop.0, sample_rate);
        if window_samples == 0 || hop_samples == 0 {
            return Err(AnalysisError::InvalidConfig(
                "frame window and hop must be at least one sample".to_owned(),
            ));
        }
        if window_samples < hop_samples {
            return Err(AnalysisError::InvalidConfig(
                "frame window must be at least as long as the hop".to_owned(),
            ));
        }
        Ok(Self {
            config,
            sample_rate,
            window_samples,
            hop_samples,
            buffer: Vec::with_capacity(window_samples + hop_samples),
            buffer_start: 0,
            next_hop: 0,
            total_samples: 0,
        })
    }

    /// Number of samples in one measurement window.
    #[must_use]
    pub const fn window_samples(&self) -> usize {
        self.window_samples
    }

    /// Number of samples between consecutive frames.
    #[must_use]
    pub const fn hop_samples(&self) -> usize {
        self.hop_samples
    }

    /// Frame timing used by this analyzer.
    #[must_use]
    pub const fn config(&self) -> &FrameConfig {
        &self.config
    }

    /// Consumes a PCM chunk and returns every frame whose full measurement
    /// window is now available.
    #[must_use]
    pub fn push(&mut self, samples: &[f32]) -> Vec<AudioFrame> {
        self.total_samples += samples.len() as u64;
        self.buffer.extend_from_slice(samples);

        let mut emitted = Vec::new();
        loop {
            let frame_start = self.next_hop;
            let frame_end = frame_start.saturating_add(self.window_samples as u64);
            if self.buffer_start.saturating_add(self.buffer.len() as u64) < frame_end {
                break;
            }
            let from = usize::try_from(frame_start - self.buffer_start).unwrap_or(usize::MAX);
            let (rms, peak) = measure(&self.buffer[from..from + self.window_samples]);
            emitted.push(self.make_frame(frame_start, rms, peak, self.window_samples));
            self.next_hop += self.hop_samples as u64;
        }
        self.compact();
        emitted
    }

    /// Emits the trailing frames whose windows are only partially filled and
    /// verifies that no samples were lost.
    #[must_use]
    pub fn finish(mut self) -> Vec<AudioFrame> {
        let mut emitted = Vec::new();
        while self.next_hop < self.total_samples {
            let frame_start = self.next_hop;
            let from = usize::try_from(frame_start - self.buffer_start).unwrap_or(usize::MAX);
            let covered = self.buffer.len() - from;
            if covered == 0 {
                break;
            }
            let (rms, peak) = measure(&self.buffer[from..]);
            emitted.push(self.make_frame(frame_start, rms, peak, covered));
            self.next_hop += self.hop_samples as u64;
        }
        emitted
    }

    fn make_frame(
        &self,
        frame_start: u64,
        rms: f64,
        peak: f64,
        window_covered: usize,
    ) -> AudioFrame {
        let start = Rational::new(i128::from(frame_start), i128::from(self.sample_rate))
            .expect("sample rate is positive");
        let hop_covered = self
            .hop_samples
            .min(usize::try_from(self.total_samples - frame_start).unwrap_or(usize::MAX));
        let duration = Rational::new(usize_to_i128(hop_covered), i128::from(self.sample_rate))
            .expect("sample rate is positive");
        let window_duration =
            Rational::new(usize_to_i128(window_covered), i128::from(self.sample_rate))
                .expect("sample rate is positive");
        let index = frame_start / u64::try_from(self.hop_samples).unwrap_or(1);
        AudioFrame {
            index,
            start,
            duration,
            window_duration,
            rms_dbfs: rms,
            peak_dbfs: peak,
        }
    }

    fn compact(&mut self) {
        let keep = usize::try_from(self.next_hop - self.buffer_start).unwrap_or(usize::MAX);
        let keep = keep.min(self.buffer.len());
        if keep > 0 {
            self.buffer.drain(..keep);
            self.buffer_start += keep as u64;
        }
    }
}

/// Builds all frames from an iterator of PCM chunks.
///
/// # Errors
///
/// Propagates invalid frame configuration from [`FrameAnalyzer::new`].
pub fn frames_from_chunks<I, C>(
    chunks: I,
    sample_rate: u32,
    config: &FrameConfig,
) -> Result<Vec<AudioFrame>, AnalysisError>
where
    I: IntoIterator<Item = C>,
    C: AsRef<[f32]>,
{
    let mut analyzer = FrameAnalyzer::new(sample_rate, *config)?;
    let mut frames = Vec::new();
    for chunk in chunks {
        frames.extend(analyzer.push(chunk.as_ref()));
    }
    frames.extend(analyzer.finish());
    Ok(frames)
}

/// Builds all frames from one contiguous PCM slice.
///
/// # Errors
///
/// Propagates invalid frame configuration from [`FrameAnalyzer::new`].
pub fn frames_from_samples(
    samples: &[f32],
    sample_rate: u32,
    config: &FrameConfig,
) -> Result<Vec<AudioFrame>, AnalysisError> {
    frames_from_chunks(std::iter::once(samples), sample_rate, config)
}

#[cfg(test)]
#[allow(
    clippy::assert_is_empty,
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    reason = "test fixtures use small, known-safe numeric conversions"
)]
mod tests {
    use super::*;

    #[test]
    fn default_frame_config_is_30ms_over_10ms() {
        let config = FrameConfig::default();
        assert_eq!(config.window.0, Rational::new(3, 100).unwrap());
        assert_eq!(config.hop.0, Rational::new(1, 100).unwrap());
        assert_eq!(duration_to_samples(config.window.0, 16_000), 480);
        assert_eq!(duration_to_samples(config.hop.0, 16_000), 160);
    }

    #[test]
    fn rejects_zero_sample_rate_and_sub_sample_frames() {
        assert!(FrameAnalyzer::new(0, FrameConfig::default()).is_err());
        let tiny = FrameConfig {
            window: Seconds(Rational::new(1, 100_000).expect("constant fraction")),
            hop: Seconds(Rational::new(1, 100_000).expect("constant fraction")),
        };
        assert!(FrameAnalyzer::new(16_000, tiny).is_err());
    }

    #[test]
    fn emits_expected_frames_for_one_second_of_silence() {
        let frames =
            frames_from_samples(&vec![0.0_f32; 16_000], 16_000, &FrameConfig::default()).unwrap();
        assert_eq!(frames.len(), 100);
        assert_eq!(frames[0].index, 0);
        assert_eq!(frames[0].start, Rational::ZERO);
        assert_eq!(frames[99].start, Rational::new(15_840, 16_000).unwrap());
        assert_eq!(frames[0].rms_dbfs, DBFS_FLOOR);
        assert_eq!(frames[0].peak_dbfs, DBFS_FLOOR);
        // Frames 0..=97 hold a full 30 ms window; the last two are partial.
        assert!(
            frames[..98]
                .iter()
                .all(|frame| frame.window_duration == Rational::new(3, 100).unwrap())
        );
        assert_eq!(frames[98].window_duration, Rational::new(1, 50).unwrap());
        assert_eq!(frames[99].window_duration, Rational::new(1, 100).unwrap());
    }

    #[test]
    fn measures_known_sine_amplitude() {
        // A full-scale sine has RMS = 1/sqrt(2) and peak = 1.
        let sample_rate = 16_000_u32;
        let mut samples = Vec::with_capacity(sample_rate as usize);
        #[allow(clippy::cast_precision_loss, reason = "test sample generation")]
        for i in 0..sample_rate {
            let phase = 2.0 * std::f64::consts::PI * 440.0 * i as f64 / sample_rate as f64;
            #[allow(clippy::cast_possible_truncation, reason = "sine is within [-1,1]")]
            samples.push(phase.sin() as f32);
        }
        let config = FrameConfig {
            window: Seconds(Rational::new(1, 1).unwrap()),
            hop: Seconds(Rational::new(1, 1).unwrap()),
        };
        let frames = frames_from_samples(&samples, sample_rate, &config).unwrap();
        assert_eq!(frames.len(), 1);
        let expected_rms = amplitude_to_dbfs(2.0_f64.sqrt().recip());
        assert!(
            (frames[0].rms_dbfs - expected_rms).abs() < 0.2,
            "{:?}",
            frames[0]
        );
        assert!((frames[0].peak_dbfs - 0.0).abs() < 0.2, "{:?}", frames[0]);
    }

    #[test]
    fn streaming_matches_one_shot() {
        let samples: Vec<f32> = (0..25_000)
            .map(|i| if i % 997 < 400 { 0.4 } else { 0.0 })
            .collect();
        let expected = frames_from_samples(&samples, 16_000, &FrameConfig::default()).unwrap();
        let chunked: Vec<Vec<f32>> = samples.chunks(733).map(<[f32]>::to_vec).collect();
        let actual = frames_from_chunks(chunked, 16_000, &FrameConfig::default()).unwrap();
        assert_eq!(expected, actual);
        assert!(!expected.is_empty());
    }

    #[test]
    fn short_input_yields_single_partial_frame() {
        let frames = frames_from_samples(&[0.5; 100], 16_000, &FrameConfig::default()).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0].window_duration,
            Rational::new(100, 16_000).unwrap()
        );
        assert_eq!(frames[0].duration, Rational::new(100, 16_000).unwrap());
    }
}

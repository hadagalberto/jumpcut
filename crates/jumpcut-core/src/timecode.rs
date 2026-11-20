//! SMPTE timecode formatting and parsing, including NTSC drop-frame.
//!
//! Drop-frame timecode skips the labels `HH:MM:SS;00` and `;01` at the start
//! of every minute except every tenth minute. That makes the label rate match
//! the real 29.97 / 59.94 fps rate without accumulating drift. This module
//! counts *real* frames and converts to and from labels exactly.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::rational::{FrameRate, Rational};

/// Errors produced while building or parsing a [`Timecode`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TimecodeError {
    /// Drop-frame counting was requested for a rate that cannot use it.
    #[error("drop-frame timecode requires 29.97, 59.94 or 119.88 fps, got {0}")]
    InvalidDropFrameRate(FrameRate),
    /// A timecode component was outside its legal range.
    #[error("invalid timecode component in `{0}`")]
    InvalidComponent(String),
    /// A duration was negative and therefore has no timecode.
    #[error("timecode cannot represent a negative duration")]
    NegativeDuration,
    /// A timecode label was not in `HH:MM:SS:FF` / `HH:MM:SS;FF` form.
    #[error("invalid timecode format `{0}`")]
    InvalidFormat(String),
}

/// A timecode value: an exact frame count plus its rate and counting mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timecode {
    frame_rate: FrameRate,
    frames: u64,
    drop_frame: bool,
}

impl Timecode {
    /// Builds a timecode from a count of elapsed frames.
    ///
    /// # Errors
    ///
    /// Returns [`TimecodeError::InvalidDropFrameRate`] if `drop_frame` is true
    /// for a rate other than 29.97 / 59.94 / 119.88.
    pub fn new(
        frames: u64,
        frame_rate: FrameRate,
        drop_frame: bool,
    ) -> Result<Self, TimecodeError> {
        if drop_frame && frame_rate.drop_frames_per_minute().is_none() {
            return Err(TimecodeError::InvalidDropFrameRate(frame_rate));
        }
        Ok(Self {
            frame_rate,
            frames,
            drop_frame,
        })
    }

    /// Builds a timecode from an exact duration, rounding to the closest frame.
    ///
    /// # Errors
    ///
    /// Returns [`TimecodeError`] for negative durations or invalid drop-frame
    /// rates.
    pub fn from_seconds(
        seconds: Rational,
        frame_rate: FrameRate,
        drop_frame: bool,
    ) -> Result<Self, TimecodeError> {
        if seconds.is_negative() {
            return Err(TimecodeError::NegativeDuration);
        }
        let frames = u64::try_from(seconds.frame_count(frame_rate))
            .map_err(|_| TimecodeError::NegativeDuration)?;
        Self::new(frames, frame_rate, drop_frame)
    }

    /// The frame rate used by this timecode.
    pub const fn frame_rate(self) -> FrameRate {
        self.frame_rate
    }

    /// The exact count of elapsed frames.
    pub const fn frames(self) -> u64 {
        self.frames
    }

    /// Whether this timecode uses drop-frame counting.
    pub const fn is_drop_frame(self) -> bool {
        self.drop_frame
    }

    /// Decomposes into `(hours, minutes, seconds, frames)` labels.
    pub fn components(self) -> (u64, u64, u64, u64) {
        let nominal = u64::from(self.frame_rate.nominal_fps());
        if !self.drop_frame {
            let minutes = self.frames / (nominal * 60);
            let seconds = (self.frames / nominal) % 60;
            let frames = self.frames % nominal;
            return (minutes / 60 % 24, minutes % 60, seconds, frames);
        }

        let drop = u64::from(self.frame_rate.drop_frames_per_minute().unwrap_or(0));
        let frames_per_nominal_minute = nominal * 60;
        let frames_per_dropped_minute = frames_per_nominal_minute - drop;
        let frames_per_10_minutes = frames_per_nominal_minute * 10 - drop * 9;
        let blocks = self.frames / frames_per_10_minutes;
        let remainder = self.frames % frames_per_10_minutes;

        // The first minute of each ten-minute block never drops labels; all
        // other minutes emit `nominal * 60 - drop` labels, starting at `drop`.
        let (minute_in_block, position) = if remainder < frames_per_nominal_minute {
            (0_u64, remainder)
        } else {
            let after_first = remainder - frames_per_nominal_minute;
            (
                1 + after_first / frames_per_dropped_minute,
                after_first % frames_per_dropped_minute,
            )
        };

        let total_minutes = blocks * 10 + minute_in_block;
        // Dropped minutes restart at frame `drop` in label space, so add it
        // back before splitting into seconds/frames. Without this, 59.94 fps
        // could format an impossible label such as `;62`.
        let label_frame = position + if minute_in_block == 0 { 0 } else { drop };
        let seconds = label_frame / nominal;
        let frames = label_frame % nominal;
        (total_minutes / 60 % 24, total_minutes % 60, seconds, frames)
    }

    /// Formats as `HH:MM:SS:FF` (non-drop) or `HH:MM:SS;FF` (drop-frame).
    pub fn format(self) -> String {
        let (hours, minutes, seconds, frames) = self.components();
        let separator = if self.drop_frame { ';' } else { ':' };
        format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{frames:02}")
    }

    /// Parses a timecode label. Drop-frame labels use `;` before the frame
    /// field; non-drop labels normally use `:`, but `;` is also accepted.
    ///
    /// # Errors
    ///
    /// Returns [`TimecodeError`] for malformed text, out-of-range components,
    /// forbidden drop-frame labels or an invalid drop-frame rate.
    pub fn parse(
        text: &str,
        frame_rate: FrameRate,
        drop_frame: bool,
    ) -> Result<Self, TimecodeError> {
        let trimmed = text.trim();
        let bytes = trimmed.as_bytes();
        if !trimmed.is_ascii()
            || bytes.len() != 11
            || bytes[2] != b':'
            || bytes[5] != b':'
            || (bytes[8] != b':' && bytes[8] != b';')
        {
            return Err(TimecodeError::InvalidFormat(text.to_owned()));
        }
        let field = |start: usize, end: usize| -> Result<u32, TimecodeError> {
            let slice = &trimmed[start..end];
            if slice.len() != 2 || !slice.bytes().all(|b| b.is_ascii_digit()) {
                return Err(TimecodeError::InvalidComponent(trimmed.to_owned()));
            }
            slice
                .parse::<u32>()
                .map_err(|_| TimecodeError::InvalidComponent(trimmed.to_owned()))
        };
        let hours = field(0, 2)?;
        let minutes = field(3, 5)?;
        let seconds = field(6, 8)?;
        let frames = field(9, 11)?;

        let nominal = frame_rate.nominal_fps();
        if hours >= 24 || minutes >= 60 || seconds >= 60 || frames >= nominal {
            return Err(TimecodeError::InvalidComponent(trimmed.to_owned()));
        }

        if drop_frame {
            let drop = frame_rate
                .drop_frames_per_minute()
                .ok_or(TimecodeError::InvalidDropFrameRate(frame_rate))?;
            if minutes % 10 != 0 && seconds == 0 && frames < drop {
                return Err(TimecodeError::InvalidComponent(trimmed.to_owned()));
            }
            let total_minutes = u64::from(hours) * 60 + u64::from(minutes);
            let skipped_minutes = if total_minutes % 10 == 0 {
                total_minutes.saturating_sub(1) - total_minutes.saturating_sub(1) / 10
            } else {
                total_minutes - total_minutes / 10
            };
            let nominal_index =
                (total_minutes * 60 + u64::from(seconds)) * u64::from(nominal) + u64::from(frames);
            let frame_number = nominal_index
                .checked_sub(u64::from(drop) * skipped_minutes)
                .ok_or_else(|| TimecodeError::InvalidComponent(trimmed.to_owned()))?;
            Self::new(frame_number, frame_rate, true)
        } else {
            let total_minutes = u64::from(hours) * 60 + u64::from(minutes);
            let frame_number =
                (total_minutes * 60 + u64::from(seconds)) * u64::from(nominal) + u64::from(frames);
            Self::new(frame_number, frame_rate, false)
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

impl FromStr for Timecode {
    type Err = TimecodeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text, FrameRate::NTSC_2997, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn non_drop_frame_24_fps() {
        let tc = Timecode::new(86_400, FrameRate::FPS_24, false).unwrap();
        assert_eq!(tc.components(), (1, 0, 0, 0));
        assert_eq!(tc.format(), "01:00:00:00");
    }

    #[test]
    fn non_drop_frame_30_fps_rollover() {
        let tc = Timecode::new(1_800, FrameRate::FPS_30, false).unwrap();
        assert_eq!(tc.format(), "00:01:00:00");
        assert!(!tc.is_drop_frame());
    }

    #[test]
    fn drop_frame_boundary_from_spec() {
        let last = Timecode::new(1_799, FrameRate::NTSC_2997, true).unwrap();
        assert_eq!(last.format(), "00:00:59;29");
        let next = Timecode::new(1_800, FrameRate::NTSC_2997, true).unwrap();
        assert_eq!(next.format(), "00:01:00;02");
    }

    #[test]
    fn drop_frame_skips_two_labels_each_minute_except_every_tenth() {
        let rate = FrameRate::NTSC_2997;
        assert_eq!(
            Timecode::new(3_598, rate, true).unwrap().format(),
            "00:02:00;02"
        );
        assert_eq!(
            Timecode::new(17_981, rate, true).unwrap().format(),
            "00:09:59;29"
        );
        assert_eq!(
            Timecode::new(17_982, rate, true).unwrap().format(),
            "00:10:00;00"
        );
        assert_eq!(
            Timecode::new(17_984, rate, true).unwrap().format(),
            "00:10:00;02"
        );
        assert_eq!(
            Timecode::new(179_820, rate, true).unwrap().format(),
            "01:40:00;00"
        );
    }

    #[test]
    fn drop_frame_59_94_skips_four_labels() {
        let rate = FrameRate::NTSC_5994;
        assert_eq!(
            Timecode::new(3_599, rate, true).unwrap().format(),
            "00:00:59;59"
        );
        assert_eq!(
            Timecode::new(3_600, rate, true).unwrap().format(),
            "00:01:00;04"
        );
        assert_eq!(
            Timecode::new(35_964, rate, true).unwrap().format(),
            "00:10:00;00"
        );
    }

    #[test]
    fn parses_labels_back_to_frames() {
        let rate = FrameRate::NTSC_2997;
        for (label, frames) in [
            ("00:00:00;00", 0),
            ("00:00:59;29", 1_799),
            ("00:01:00;02", 1_800),
            ("00:02:00;02", 3_598),
            ("00:09:59;29", 17_981),
            ("00:10:00;00", 17_982),
            ("01:40:00;00", 179_820),
        ] {
            let parsed = Timecode::parse(label, rate, true).unwrap();
            assert_eq!(parsed.frames(), frames, "label {label}");
            assert_eq!(parsed.format(), label);
        }
    }

    #[test]
    fn rejects_forbidden_and_malformed_labels() {
        let rate = FrameRate::NTSC_2997;
        assert!(Timecode::parse("00:01:00;00", rate, true).is_err());
        assert!(Timecode::parse("00:01:00;01", rate, true).is_err());
        assert!(Timecode::parse("00:01:00;30", rate, true).is_err());
        assert!(Timecode::parse("00:60:00;02", rate, true).is_err());
        assert!(Timecode::parse("00:01.00;02", rate, true).is_err());
        assert!(Timecode::parse("bad", rate, true).is_err());
        assert!(Timecode::parse("01:00:00:00", rate, true).is_ok());
    }

    #[test]
    fn rejects_drop_frame_on_integer_rates() {
        assert!(Timecode::new(0, FrameRate::FPS_25, true).is_err());
        assert!(Timecode::parse("00:00:00;00", FrameRate::FPS_30, true).is_err());
    }

    #[test]
    fn builds_from_exact_seconds() {
        let tc = Timecode::from_seconds(Rational::new(1, 1).unwrap(), FrameRate::NTSC_2997, true)
            .unwrap();
        assert_eq!(tc.frames(), 30);
        assert_eq!(tc.format(), "00:00:01;00");
        assert!(
            Timecode::from_seconds(Rational::new(-1, 1).unwrap(), FrameRate::FPS_25, false)
                .is_err()
        );
    }

    #[test]
    fn display_matches_format() {
        let tc = Timecode::new(1_800, FrameRate::NTSC_2997, true).unwrap();
        assert_eq!(tc.to_string(), "00:01:00;02");
        assert_eq!("00:01:00;02".parse::<Timecode>().unwrap(), tc);
    }

    proptest::proptest! {
        #[test]
        fn format_parse_round_trip_non_drop(frames in 0u64..1_600_000, rate_choice in 0u8..5) {
            let rate = match rate_choice {
                0 => FrameRate::FPS_24,
                1 => FrameRate::FPS_25,
                2 => FrameRate::FPS_30,
                3 => FrameRate::FPS_50,
                _ => FrameRate::FPS_60,
            };
            let tc = Timecode::new(frames, rate, false).unwrap();
            let round_trip = Timecode::parse(&tc.format(), rate, false).unwrap();
            prop_assert_eq!(round_trip, tc);
        }

        #[test]
        fn format_parse_round_trip_drop_frame(frames in 0u64..1_500_000, rate_choice in 0u8..2) {
            let rate = if rate_choice == 0 {
                FrameRate::NTSC_2997
            } else {
                FrameRate::NTSC_5994
            };
            let tc = Timecode::new(frames, rate, true).unwrap();
            let round_trip = Timecode::parse(&tc.format(), rate, true).unwrap();
            prop_assert_eq!(round_trip.frames(), frames);
            prop_assert!(tc.format().as_bytes()[8] == b';');
        }
    }
}

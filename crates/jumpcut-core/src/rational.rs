//! Exact rational time arithmetic.
//!
//! All timing inside jumpcut is represented as a reduced fraction of `i128`
//! integers. This keeps NTSC frame rates (`30000/1001`, `24000/1001`,
//! `60000/1001`) exact over arbitrarily long timelines: no accumulating
//! floating point drift, no "29.97" approximations.

use std::cmp::Ordering;
use std::fmt;
use std::num::ParseIntError;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors produced while constructing or parsing a [`Rational`] / [`FrameRate`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RationalError {
    /// A fraction was given with a zero denominator.
    #[error("denominator must not be zero")]
    ZeroDenominator,
    /// The text was not a valid integer, decimal or `num/den` fraction.
    #[error("invalid rational number `{0}`")]
    InvalidFormat(String),
    /// Arithmetic overflowed the 128-bit representation.
    #[error("rational arithmetic overflow")]
    Overflow,
    /// A frame rate must be strictly positive.
    #[error("frame rate must be positive, got `{0}`")]
    NonPositiveFrameRate(String),
}

impl From<ParseIntError> for RationalError {
    fn from(_: ParseIntError) -> Self {
        Self::InvalidFormat(String::new())
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// A reduced fraction `num / den` with `den > 0`.
///
/// Zero is always stored as `0/1`. Arithmetic is exact; conversions to
/// floating point happen only at the very edge (display, ffmpeg arguments).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Rational {
    num: i128,
    den: i128,
}

impl Rational {
    /// The rational number zero (`0/1`).
    pub const ZERO: Self = Self { num: 0, den: 1 };
    /// The rational number one (`1/1`).
    pub const ONE: Self = Self { num: 1, den: 1 };

    /// Creates a reduced rational number.
    ///
    /// # Errors
    ///
    /// Returns [`RationalError::ZeroDenominator`] when `den` is zero and
    /// [`RationalError::Overflow`] when the value cannot be normalised.
    pub fn new(num: i128, den: i128) -> Result<Self, RationalError> {
        if den == 0 {
            return Err(RationalError::ZeroDenominator);
        }
        let (num, den) = if den < 0 {
            (
                num.checked_neg().ok_or(RationalError::Overflow)?,
                den.checked_neg().ok_or(RationalError::Overflow)?,
            )
        } else {
            (num, den)
        };
        if num == 0 {
            return Ok(Self::ZERO);
        }
        let magnitude = num.checked_abs().ok_or(RationalError::Overflow)?;
        let divisor = gcd(magnitude.unsigned_abs(), den.unsigned_abs());
        let divisor = i128::try_from(divisor).map_err(|_| RationalError::Overflow)?;
        Ok(Self {
            num: num / divisor,
            den: den / divisor,
        })
    }

    /// Numerator of the reduced fraction.
    pub const fn num(self) -> i128 {
        self.num
    }

    /// Denominator of the reduced fraction (always positive).
    pub const fn den(self) -> i128 {
        self.den
    }

    /// Whether this value is exactly zero.
    pub const fn is_zero(self) -> bool {
        self.num == 0
    }

    /// Whether this value is strictly negative.
    pub const fn is_negative(self) -> bool {
        self.num < 0
    }

    /// Approximate value as `f64`. Use only for display or external tools.
    #[allow(
        clippy::cast_precision_loss,
        reason = "an approximation is the explicit purpose of this method"
    )]
    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Absolute value.
    #[must_use]
    pub fn abs(self) -> Self {
        Self {
            num: self.num.abs(),
            den: self.den,
        }
    }

    /// Largest integer less than or equal to this value.
    pub fn floor(self) -> i128 {
        self.num.div_euclid(self.den)
    }

    /// Nearest integer, with halves rounded away from zero.
    pub fn round(self) -> i128 {
        let quotient = self.num.div_euclid(self.den);
        let remainder = self.num.rem_euclid(self.den);
        if remainder * 2 >= self.den {
            quotient + 1
        } else {
            quotient
        }
    }

    /// Checked addition.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let den = self.den.checked_mul(rhs.den)?;
        let num = self
            .num
            .checked_mul(rhs.den)?
            .checked_add(rhs.num.checked_mul(self.den)?)?;
        Self::new(num, den).ok()
    }

    /// Checked subtraction.
    pub fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.checked_add(-rhs)
    }

    /// Checked multiplication.
    pub fn checked_mul(self, rhs: Self) -> Option<Self> {
        let num = self.num.checked_mul(rhs.num)?;
        let den = self.den.checked_mul(rhs.den)?;
        Self::new(num, den).ok()
    }

    /// Checked division.
    pub fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.is_zero() {
            return None;
        }
        let num = self.num.checked_mul(rhs.den)?;
        let den = self.den.checked_mul(rhs.num)?;
        Self::new(num, den).ok()
    }

    /// Parses `N`, `N/D` or a decimal such as `-12.005`.
    ///
    /// Decimals are converted exactly (`0.1` becomes `1/10`), which is why
    /// ffprobe duration strings can be read without float drift.
    ///
    /// # Errors
    ///
    /// Returns [`RationalError`] for malformed input or a zero denominator.
    pub fn parse(text: &str) -> Result<Self, RationalError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(RationalError::InvalidFormat(text.to_owned()));
        }
        if let Some((left, right)) = text.split_once('/') {
            let num: i128 = left
                .trim()
                .parse()
                .map_err(|_| RationalError::InvalidFormat(text.to_owned()))?;
            let den: i128 = right
                .trim()
                .parse()
                .map_err(|_| RationalError::InvalidFormat(text.to_owned()))?;
            return Self::new(num, den);
        }
        if let Some((integer, fraction)) = text.split_once('.') {
            let (sign, digits) = if let Some(rest) = integer.strip_prefix('-') {
                (-1_i128, rest)
            } else if let Some(rest) = integer.strip_prefix('+') {
                (1_i128, rest)
            } else {
                (1_i128, integer)
            };
            let whole: i128 = if digits.is_empty() {
                0
            } else {
                digits
                    .parse()
                    .map_err(|_| RationalError::InvalidFormat(text.to_owned()))?
            };
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(RationalError::InvalidFormat(text.to_owned()));
            }
            let scale = 10_i128
                .checked_pow(u32::try_from(fraction.len()).map_err(|_| RationalError::Overflow)?)
                .ok_or(RationalError::Overflow)?;
            let fraction_digits: i128 = fraction
                .parse()
                .map_err(|_| RationalError::InvalidFormat(text.to_owned()))?;
            let magnitude = whole
                .checked_mul(scale)
                .and_then(|whole| whole.checked_add(fraction_digits))
                .ok_or(RationalError::Overflow)?;
            let num = sign.checked_mul(magnitude).ok_or(RationalError::Overflow)?;
            return Self::new(num, scale);
        }
        let value: i128 = text
            .parse()
            .map_err(|_| RationalError::InvalidFormat(text.to_owned()))?;
        Self::new(value, 1)
    }

    /// Duration in seconds of `frames` frames played back at `rate`.
    ///
    /// # Panics
    ///
    /// Panics if the frame count overflows the 128-bit representation.
    pub fn from_frames(frames: i128, rate: FrameRate) -> Self {
        let numerator = frames
            .checked_mul(rate.0.den())
            .expect("frame count overflow");
        Self::new(numerator, rate.0.num()).expect("frame rate has positive numerator")
    }

    /// Number of frames at `rate` closest to this duration.
    pub fn frame_count(self, rate: FrameRate) -> i128 {
        (self * rate.0).round()
    }

    /// This duration snapped to the nearest whole frame at `rate`.
    #[must_use]
    pub fn snapped_to_frame(self, rate: FrameRate) -> Self {
        Self::from_frames(self.frame_count(rate), rate)
    }
}

impl Default for Rational {
    fn default() -> Self {
        Self::ZERO
    }
}

impl FromStr for Rational {
    type Err = RationalError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.num * other.den).cmp(&(other.num * self.den))
    }
}

impl Neg for Rational {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            num: -self.num,
            den: self.den,
        }
    }
}

impl Add for Rational {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        self.checked_add(rhs).expect("rational addition overflow")
    }
}

impl AddAssign for Rational {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Rational {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        self.checked_sub(rhs)
            .expect("rational subtraction overflow")
    }
}

impl SubAssign for Rational {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul for Rational {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        self.checked_mul(rhs)
            .expect("rational multiplication overflow")
    }
}

impl Div for Rational {
    type Output = Self;

    fn div(self, rhs: Self) -> Self::Output {
        self.checked_div(rhs)
            .expect("rational division by zero or overflow")
    }
}

impl From<i64> for Rational {
    fn from(value: i64) -> Self {
        Self {
            num: i128::from(value),
            den: 1,
        }
    }
}

/// A video frame rate stored as an exact fraction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct FrameRate(pub(crate) Rational);

impl FrameRate {
    /// Constant frame rate: 24000/1001 (23.976 fps).
    pub const NTSC_23976: Self = Self(Rational {
        num: 24_000,
        den: 1_001,
    });
    /// Constant frame rate: 30000/1001 (29.97 fps).
    pub const NTSC_2997: Self = Self(Rational {
        num: 30_000,
        den: 1_001,
    });
    /// Constant frame rate: 48000/1001 (47.952 fps).
    pub const NTSC_47952: Self = Self(Rational {
        num: 48_000,
        den: 1_001,
    });
    /// Constant frame rate: 60000/1001 (59.94 fps).
    pub const NTSC_5994: Self = Self(Rational {
        num: 60_000,
        den: 1_001,
    });
    /// Constant frame rate: 24 fps.
    pub const FPS_24: Self = Self(Rational { num: 24, den: 1 });
    /// Constant frame rate: 25 fps.
    pub const FPS_25: Self = Self(Rational { num: 25, den: 1 });
    /// Constant frame rate: 30 fps.
    pub const FPS_30: Self = Self(Rational { num: 30, den: 1 });
    /// Constant frame rate: 50 fps.
    pub const FPS_50: Self = Self(Rational { num: 50, den: 1 });
    /// Constant frame rate: 60 fps.
    pub const FPS_60: Self = Self(Rational { num: 60, den: 1 });

    /// Builds an arbitrary frame rate from an exact numerator/denominator.
    ///
    /// # Errors
    ///
    /// Returns [`RationalError`] when the rate is not strictly positive.
    pub fn new(num: i128, den: i128) -> Result<Self, RationalError> {
        let rate = Rational::new(num, den)?;
        if rate.num <= 0 {
            return Err(RationalError::NonPositiveFrameRate(rate.to_string()));
        }
        Ok(Self(rate))
    }

    /// Parses a frame rate. Decimal spellings of NTSC rates are mapped to
    /// their exact rational equivalents (`"29.97"` -> `30000/1001`).
    ///
    /// # Errors
    ///
    /// Returns [`RationalError`] for invalid or non-positive rates.
    pub fn parse(text: &str) -> Result<Self, RationalError> {
        match text.trim() {
            "23.976" | "24000/1001" => Ok(Self::NTSC_23976),
            "29.97" | "30000/1001" => Ok(Self::NTSC_2997),
            "47.952" | "48000/1001" => Ok(Self::NTSC_47952),
            "59.94" | "60000/1001" => Ok(Self::NTSC_5994),
            other => Self::new_from_rational(Rational::parse(other)?),
        }
    }

    fn new_from_rational(rate: Rational) -> Result<Self, RationalError> {
        if rate.num <= 0 {
            return Err(RationalError::NonPositiveFrameRate(rate.to_string()));
        }
        Ok(Self(rate))
    }

    /// Exact rate as a rational number of frames per second.
    pub const fn as_rational(self) -> Rational {
        self.0
    }

    /// Nominal integer frame rate: the rounded-up label rate (30 for 29.97).
    pub fn nominal_fps(self) -> u32 {
        u32::try_from(self.0.round()).unwrap_or(u32::MAX)
    }

    /// Whether this is an NTSC (non-integer) frame rate such as 29.97.
    #[must_use]
    pub const fn is_ntsc(self) -> bool {
        self.0.den() != 1
    }

    /// Number of timecode labels dropped per minute for drop-frame timecode.
    ///
    /// Returns `None` when this rate is integer-locked and therefore cannot
    /// use drop-frame counting.
    pub fn drop_frames_per_minute(self) -> Option<u32> {
        let nominal = self.nominal_fps();
        if nominal % 30 != 0 || self.0 == Rational::new(i128::from(nominal), 1).ok()? {
            return None;
        }
        Some(nominal / 15)
    }

    /// Duration of `frames` frames at this rate.
    pub fn duration_of_frames(self, frames: i128) -> Rational {
        Rational::from_frames(frames, self)
    }
}

impl FromStr for FrameRate {
    type Err = RationalError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl fmt::Display for FrameRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn r(num: i128, den: i128) -> Rational {
        Rational::new(num, den).unwrap()
    }

    #[test]
    fn normalises_signs_and_zero() {
        assert_eq!(r(2, 4), r(1, 2));
        assert_eq!(r(-2, -4), r(1, 2));
        assert_eq!(r(-2, 4), r(1, -2));
        assert_eq!(r(-2, 4), r(-1, 2));
        assert_eq!(r(0, 99).den(), 1);
        assert_eq!(Rational::new(1, 0), Err(RationalError::ZeroDenominator));
    }

    #[test]
    fn exact_arithmetic() {
        assert_eq!(r(1, 3) + r(1, 6), r(1, 2));
        assert_eq!(r(3, 4) - r(1, 4), r(1, 2));
        assert_eq!(r(2, 3) * r(9, 4), r(3, 2));
        assert_eq!(r(2, 3) / r(4, 9), r(3, 2));
        assert_eq!(-r(2, 5), r(-2, 5));
        assert!(r(1, 2) < r(3, 5));
        assert!(r(-1, 2) > r(-3, 5));
    }

    #[test]
    fn ntsc_rates_are_exact() {
        assert_eq!(FrameRate::NTSC_2997.as_rational(), r(30_000, 1_001));
        assert_eq!(FrameRate::NTSC_2997.duration_of_frames(30_000), r(1_001, 1));
        assert_eq!(
            FrameRate::NTSC_23976.duration_of_frames(24_000),
            r(1_001, 1)
        );
        assert_eq!(FrameRate::NTSC_5994.duration_of_frames(60_000), r(1_001, 1));
        assert_eq!(FrameRate::NTSC_2997.nominal_fps(), 30);
        assert_eq!(FrameRate::NTSC_2997.drop_frames_per_minute(), Some(2));
        assert_eq!(FrameRate::NTSC_5994.drop_frames_per_minute(), Some(4));
        assert_eq!(FrameRate::FPS_30.drop_frames_per_minute(), None);
        assert_eq!(FrameRate::FPS_25.drop_frames_per_minute(), None);
        assert!(FrameRate::NTSC_23976.is_ntsc());
        assert!(FrameRate::NTSC_2997.is_ntsc());
        assert!(FrameRate::NTSC_5994.is_ntsc());
        assert!(!FrameRate::FPS_30.is_ntsc());
        assert!(!FrameRate::FPS_25.is_ntsc());
    }

    #[test]
    fn parses_rate_spellings_without_float_drift() {
        assert_eq!(FrameRate::parse("29.97").unwrap(), FrameRate::NTSC_2997);
        assert_eq!(FrameRate::parse("23.976").unwrap(), FrameRate::NTSC_23976);
        assert_eq!(FrameRate::parse("59.94").unwrap(), FrameRate::NTSC_5994);
        assert_eq!(
            FrameRate::parse("30000/1001").unwrap(),
            FrameRate::NTSC_2997
        );
        assert_eq!(FrameRate::parse("25").unwrap(), FrameRate::FPS_25);
        assert!(FrameRate::parse("0").is_err());
        assert!(FrameRate::parse("-25").is_err());
    }

    #[test]
    fn parses_decimal_durations_exactly() {
        assert_eq!(Rational::parse("90.09").unwrap(), r(9_009, 100));
        assert_eq!(Rational::parse("-0.5").unwrap(), r(-1, 2));
        assert_eq!(Rational::parse("1001/1000").unwrap(), r(1_001, 1_000));
        assert_eq!(Rational::parse("42").unwrap(), r(42, 1));
        assert!(Rational::parse("N/A").is_err());
        assert!(Rational::parse("1.").is_err());
        assert!(Rational::parse("1/0").is_err());
    }

    #[test]
    fn long_timeline_does_not_drift() {
        // Two hours at 29.97 fps: 216000000/1001 seconds exactly; 215784
        // whole frames, not 215784.216 as a float would round.
        let two_hours = r(7_200, 1);
        let frames = (two_hours * FrameRate::NTSC_2997.as_rational()).round();
        assert_eq!(frames, 215_784);
        let snapped = two_hours.snapped_to_frame(FrameRate::NTSC_2997);
        assert_eq!(snapped.frame_count(FrameRate::NTSC_2997), 215_784);
        assert!(snapped <= two_hours);
    }

    #[test]
    fn rounding_is_half_away_from_zero_for_positive_values() {
        assert_eq!(r(1, 2).round(), 1);
        assert_eq!(r(2, 3).round(), 1);
        assert_eq!(r(1, 3).round(), 0);
        assert_eq!(r(7, 2).floor(), 3);
    }

    proptest! {
        #[test]
        fn reduced_form_is_canonical(num in -10_000i128..10_000, den in -10_000i128..10_000) {
            prop_assume!(den != 0);
            let value = r(num, den);
            prop_assert!(value.den() > 0);
            if value.is_zero() {
                prop_assert_eq!(value.den(), 1);
            } else {
                prop_assert_eq!(
                    gcd(value.num().unsigned_abs(), value.den().unsigned_abs()),
                    1
                );
            }
        }

        #[test]
        fn addition_is_commutative_and_associative(
            a in -1_000i128..1_000, b in -1_000i128..1_000,
            c in 1i128..1_000, d in 1i128..1_000, e in 1i128..1_000
        ) {
            let (x, y, z) = (r(a, c), r(b, d), r(c, e));
            prop_assert_eq!(x + y, y + x);
            prop_assert_eq!((x + y) + z, x + (y + z));
            prop_assert_eq!(x - y, -(y - x));
        }

        #[test]
        fn multiplication_and_division_round_trip(
            a in -1_000i128..1_000, b in 1i128..1_000,
            c in -1_000i128..1_000, d in 1i128..1_000
        ) {
            let (x, y) = (r(a, b), r(c, d));
            prop_assume!(!y.is_zero());
            prop_assert_eq!((x * y) / y, x);
            prop_assert_eq!(x * Rational::ONE, x);
        }

        #[test]
        fn ordering_matches_cross_multiplication(
            a in -1_000i128..1_000, b in 1i128..1_000,
            c in -1_000i128..1_000, d in 1i128..1_000
        ) {
            let (x, y) = (r(a, b), r(c, d));
            prop_assert_eq!(x < y, a * d < c * b);
            prop_assert_eq!(x == y, a * d == c * b);
        }

        #[test]
        fn snapping_lands_on_a_frame(seconds in 0i128..10_000, den in 1i128..1_000) {
            let time = r(seconds, den);
            for rate in [
                FrameRate::FPS_24,
                FrameRate::FPS_25,
                FrameRate::NTSC_2997,
                FrameRate::NTSC_5994,
            ] {
                let snapped = time.snapped_to_frame(rate);
                let frames = snapped.frame_count(rate);
                prop_assert_eq!(snapped, Rational::from_frames(frames, rate));
                let error = (time - snapped).abs();
                prop_assert!(error <= rate.duration_of_frames(1) / Rational::new(2, 1).unwrap());
            }
        }
    }
}

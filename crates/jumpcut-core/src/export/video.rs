//! Video characteristics shared by the timeline exporters.
//!
//! Dimensions, colour metadata and field order come from ffprobe. They drive
//! the predefined FCPXML format name, the Premiere sample characteristics and
//! the optional metadata carried by other formats.

use crate::ffprobe::VideoStream;
use crate::rational::FrameRate;

/// Interlacing mode reported by the source stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scan {
    /// Full frames, all fields captured at the same instant.
    Progressive,
    /// Two interlaced fields per frame.
    Interlaced,
}

impl Scan {
    /// Single-letter FCPXML format-name marker.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Progressive => "p",
            Self::Interlaced => "i",
        }
    }
}

/// FCPXML name used when resolution or rate has no predefined format.
const RATE_UNDEFINED: &str = "FFVideoFormatRateUndefined";

/// Rate suffixes Apple publishes for progressive HD/UHD formats.
const HD_RATES: &[&str] = &["2398", "24", "25", "2997", "30", "50", "5994", "60"];
/// Rate suffixes published for progressive SD formats.
const SD_RATES: &[&str] = &["2398", "24", "25", "2997", "30"];
/// Frame-rate suffixes that have published interlaced variants (field rate is double).
const INTERLACED_RATES: &[&str] = &["25", "2997"];

/// One resolution family from Apple's predefined FCPXML format table.
struct FormatClass {
    width: u32,
    height: u32,
    /// Prefix between `FFVideoFormat` and the scan marker.
    prefix: &'static str,
    progressive: &'static [&'static str],
    interlaced: &'static [&'static str],
}

/// Predefined formats from the Apple FCPXML 1.10 reference.
const FORMAT_CLASSES: &[FormatClass] = &[
    FormatClass {
        width: 1920,
        height: 1080,
        prefix: "1080",
        progressive: HD_RATES,
        interlaced: INTERLACED_RATES,
    },
    FormatClass {
        width: 1280,
        height: 720,
        prefix: "720",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 1280,
        height: 1080,
        prefix: "1280x1080",
        progressive: HD_RATES,
        interlaced: INTERLACED_RATES,
    },
    FormatClass {
        width: 1440,
        height: 1080,
        prefix: "1440x1080",
        progressive: HD_RATES,
        interlaced: INTERLACED_RATES,
    },
    FormatClass {
        width: 2048,
        height: 1024,
        prefix: "2048x1024",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 2048,
        height: 1080,
        prefix: "2048x1080",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 2048,
        height: 1152,
        prefix: "2048x1152",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 2048,
        height: 1536,
        prefix: "2048x1536",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 2048,
        height: 1556,
        prefix: "2048x1556",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 3840,
        height: 2160,
        prefix: "3840x2160",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 4096,
        height: 2048,
        prefix: "4096x2048",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 4096,
        height: 2160,
        prefix: "4096x2160",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 4096,
        height: 2304,
        prefix: "4096x2304",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 4096,
        height: 3112,
        prefix: "4096x3112",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 5120,
        height: 2160,
        prefix: "5120x2160",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 5120,
        height: 2560,
        prefix: "5120x2560",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 5120,
        height: 2700,
        prefix: "5120x2700",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 640,
        height: 480,
        prefix: "640x480",
        progressive: SD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 640,
        height: 360,
        prefix: "640x360",
        progressive: &["2997"],
        interlaced: &[],
    },
    FormatClass {
        width: 960,
        height: 540,
        prefix: "960x540",
        progressive: SD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 960,
        height: 720,
        prefix: "960x720",
        progressive: HD_RATES,
        interlaced: &[],
    },
    FormatClass {
        width: 720,
        height: 486,
        prefix: "720x486",
        progressive: &["2398", "2997"],
        interlaced: &["2997"],
    },
    FormatClass {
        width: 720,
        height: 576,
        prefix: "720x576",
        progressive: &["25"],
        interlaced: &["25"],
    },
];

fn rate_key(rate: FrameRate) -> Option<&'static str> {
    if rate == FrameRate::NTSC_23976 {
        Some("2398")
    } else if rate == FrameRate::FPS_24 {
        Some("24")
    } else if rate == FrameRate::FPS_25 {
        Some("25")
    } else if rate == FrameRate::NTSC_2997 {
        Some("2997")
    } else if rate == FrameRate::FPS_30 {
        Some("30")
    } else if rate == FrameRate::FPS_50 {
        Some("50")
    } else if rate == FrameRate::NTSC_5994 {
        Some("5994")
    } else if rate == FrameRate::FPS_60 {
        Some("60")
    } else {
        None
    }
}

/// Interlaced format names label the field rate, twice the frame rate.
fn field_key(frame_rate_key: &'static str) -> &'static str {
    match frame_rate_key {
        "25" => "50",
        "2997" => "5994",
        other => other,
    }
}

/// Video properties that NLE exporters need from the source media.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoInfo {
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
    /// ffprobe colour-space tag, when the stream declares one.
    pub color_space: Option<String>,
    /// ffprobe colour-primaries tag, when the stream declares one.
    pub color_primaries: Option<String>,
    /// ffprobe colour-transfer tag, when the stream declares one.
    pub color_transfer: Option<String>,
    /// ffprobe field-order tag, when the stream declares one.
    pub field_order: Option<String>,
}

impl VideoInfo {
    /// Fallback used for audio-only inputs, which have no ffprobe video stream.
    pub const DEFAULT: Self = Self {
        width: 1920,
        height: 1080,
        color_space: None,
        color_primaries: None,
        color_transfer: None,
        field_order: None,
    };

    /// Copies the relevant ffprobe fields from a video stream.
    #[must_use]
    pub fn from_stream(stream: &VideoStream) -> Self {
        Self {
            width: stream.width,
            height: stream.height,
            color_space: stream.color_space.clone(),
            color_primaries: stream.color_primaries.clone(),
            color_transfer: stream.color_transfer.clone(),
            field_order: stream.field_order.clone(),
        }
    }

    /// Interlaced when ffprobe reports explicit alternating field order.
    #[must_use]
    pub fn scan(&self) -> Scan {
        match self.field_order.as_deref() {
            Some(
                "tt"
                | "bb"
                | "tb"
                | "bt"
                | "interlaced"
                | "interlaced_top_first"
                | "interlaced_bottom_first",
            ) => Scan::Interlaced,
            _ => Scan::Progressive,
        }
    }

    /// FCPXML predefined format name for these dimensions, scan and rate.
    ///
    /// Unknown resolutions, unknown rates and unsupported interlaced rates
    /// fall back to `FFVideoFormatRateUndefined` as Apple prescribes for
    /// custom formats.
    #[must_use]
    pub fn fcp_format_name(&self, rate: FrameRate) -> String {
        let Some(key) = rate_key(rate) else {
            return RATE_UNDEFINED.to_owned();
        };
        let scan = self.scan();
        for class in FORMAT_CLASSES {
            if class.width != self.width || class.height != self.height {
                continue;
            }
            let (keys, emitted) = match scan {
                Scan::Progressive => (class.progressive, key),
                Scan::Interlaced => {
                    if !class.interlaced.contains(&key) {
                        continue;
                    }
                    (class.interlaced, field_key(key))
                }
            };
            if keys.contains(&key) {
                return format!("FFVideoFormat{}{}{}", class.prefix, scan.code(), emitted);
            }
        }
        RATE_UNDEFINED.to_owned()
    }

    /// Maps a ffprobe colour-space tag to an FCPXML `colorSpace` value.
    ///
    /// Returns `None` when the stream does not declare a recognised tag.
    #[must_use]
    pub fn fcp_color_space(&self) -> Option<String> {
        match self.color_space.as_deref()?.to_ascii_lowercase().as_str() {
            "bt709" | "rec709" => Some("1-1-1 (Rec. 709)".to_owned()),
            "bt601" | "bt470bg" | "smpte170m" => Some("1-1-1 (Rec. 601)".to_owned()),
            "bt2020" | "bt2020nc" | "bt2020c" => Some("1-1-1 (Rec. 2020)".to_owned()),
            _ => None,
        }
    }

    /// FCPXML `colorSpace` value, defaulting to Rec. 709 for untagged media.
    #[must_use]
    pub fn fcp_color_space_or_default(&self) -> String {
        self.fcp_color_space()
            .unwrap_or_else(|| "1-1-1 (Rec. 709)".to_owned())
    }
}

impl Default for VideoInfo {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(width: u32, height: u32) -> VideoInfo {
        VideoInfo {
            width,
            height,
            ..VideoInfo::DEFAULT
        }
    }

    #[test]
    fn derives_progressive_format_names_from_resolution_and_rate() {
        assert_eq!(
            video(320, 240).fcp_format_name(FrameRate::NTSC_2997),
            "FFVideoFormatRateUndefined"
        );
        assert_eq!(
            video(1920, 1080).fcp_format_name(FrameRate::NTSC_23976),
            "FFVideoFormat1080p2398"
        );
        assert_eq!(
            video(3840, 2160).fcp_format_name(FrameRate::FPS_60),
            "FFVideoFormat3840x2160p60"
        );
        assert_eq!(
            video(1080, 1920).fcp_format_name(FrameRate::FPS_30),
            "FFVideoFormatRateUndefined"
        );
        assert_eq!(
            video(1280, 720).fcp_format_name(FrameRate::NTSC_2997),
            "FFVideoFormat720p2997"
        );
        assert_eq!(
            video(1920, 1080).fcp_format_name(FrameRate::NTSC_2997),
            "FFVideoFormat1080p2997"
        );
    }

    #[test]
    fn derives_interlaced_names_using_field_rate() {
        let mut interlaced = video(1920, 1080);
        interlaced.field_order = Some("tt".to_owned());
        assert_eq!(interlaced.scan(), Scan::Interlaced);
        assert_eq!(
            interlaced.fcp_format_name(FrameRate::NTSC_2997),
            "FFVideoFormat1080i5994"
        );
        assert_eq!(
            interlaced.fcp_format_name(FrameRate::FPS_25),
            "FFVideoFormat1080i50"
        );
        // Progressive-only resolutions have no interlaced predefined name.
        let mut progressive_only = video(1280, 720);
        progressive_only.field_order = Some("bb".to_owned());
        assert_eq!(
            progressive_only.fcp_format_name(FrameRate::NTSC_2997),
            "FFVideoFormatRateUndefined"
        );
    }

    #[test]
    fn unknown_rates_fall_back_to_undefined() {
        assert_eq!(
            video(1920, 1080).fcp_format_name(FrameRate::new(100, 3).unwrap()),
            "FFVideoFormatRateUndefined"
        );
    }

    #[test]
    fn maps_color_metadata_only_when_present() {
        let mut info = video(1920, 1080);
        assert_eq!(info.fcp_color_space(), None);
        assert_eq!(info.fcp_color_space_or_default(), "1-1-1 (Rec. 709)");

        info.color_space = Some("bt709".to_owned());
        assert_eq!(info.fcp_color_space().as_deref(), Some("1-1-1 (Rec. 709)"));

        info.color_space = Some("smpte170m".to_owned());
        assert_eq!(info.fcp_color_space().as_deref(), Some("1-1-1 (Rec. 601)"));

        info.color_space = Some("BT2020NC".to_owned());
        assert_eq!(info.fcp_color_space().as_deref(), Some("1-1-1 (Rec. 2020)"));

        info.color_space = Some("something-new".to_owned());
        assert_eq!(info.fcp_color_space(), None);
        assert_eq!(info.fcp_color_space_or_default(), "1-1-1 (Rec. 709)");
    }
}

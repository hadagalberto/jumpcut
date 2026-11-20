//! `OpenTimelineIO` JSON exporter.
//!
//! The result is a `Timeline.1` with video and audio tracks. Clip times are
//! OTIO `RationalTime` frame counts at the export rate; `LinearTimeWarp` effects
//! carry silent-speed retimes. Media references point at the original file.

use serde_json::{Value, json};

use crate::export::{ExportContext, ExportError, PRODUCT_NAME, file_url};

/// Builds the source video metadata attached to the OTIO timeline.
fn video_metadata(context: &ExportContext<'_>) -> Value {
    let mut metadata = json!({
        "width": context.video.width,
        "height": context.video.height,
        "scan": context.video.scan().code(),
    });
    if let Some(color_space) = context.video.fcp_color_space() {
        metadata["colorSpace"] = json!(color_space);
    }
    metadata
}

/// Renders `OpenTimelineIO` JSON for `context`.
///
/// # Errors
///
/// Currently infallible, but returns [`ExportError`] for a uniform exporter API.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let rate = context.frame_rate.as_rational().to_f64();
    let zero = rational_time(0, rate);
    let name = format!("{} - {PRODUCT_NAME}", context.source_name());
    let source_duration = context.frames(context.analysis.plan.source_duration);

    // The OTIO serialized schema puts media references in a keyed map on
    // Clip.2 (`media_references` + `active_media_reference_key`); the singular
    // `media_reference` field belongs to the older Clip.1 schema. Tracks and
    // stacks also carry the schema's effect/marker/metadata fields, and each
    // track declares its `kind`.
    let mut video = Vec::new();
    let mut audio = Vec::new();
    for clip in context.clips() {
        let clip_name = format!("{} {}", context.source_name(), clip.index + 1);
        let start = rational_time(context.frames(clip.source_start), rate);
        let duration = rational_time(context.frames(clip.record_duration()), rate);
        let source_length = rational_time(source_duration, rate);
        let mut effects = Vec::new();
        if let Some(speed) = clip.speed {
            effects.push(json!({
                "OTIO_SCHEMA": "LinearTimeWarp.1",
                "name": "jumpcut silent speed",
                "effect_name": "LinearTimeWarp",
                "enabled": true,
                "metadata": {},
                "time_scalar": speed,
            }));
        }
        let item = json!({
            "OTIO_SCHEMA": "Clip.2",
            "name": clip_name,
            "metadata": {},
            "effects": effects,
            "markers": [],
            "source_range": {
                "OTIO_SCHEMA": "TimeRange.1",
                "start_time": start,
                "duration": duration,
            },
            "media_references": {
                "DEFAULT_MEDIA": {
                    "OTIO_SCHEMA": "ExternalReference.1",
                    "name": context.source_name(),
                    "target_url": file_url(context.source),
                    "available_range": {
                        "OTIO_SCHEMA": "TimeRange.1",
                        "start_time": rational_time(0, rate),
                        "duration": source_length,
                    },
                    "metadata": {},
                },
            },
            "active_media_reference_key": "DEFAULT_MEDIA",
        });
        video.push(item.clone());
        audio.push(item);
    }

    let track = |name: &str, kind: &str, children: Vec<Value>| {
        json!({
            "OTIO_SCHEMA": "Track.1",
            "name": name,
            "kind": kind,
            "children": children,
            "effects": [],
            "markers": [],
            "metadata": {},
            "source_range": Value::Null,
        })
    };
    let timeline = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": name,
        "metadata": { "jumpcut": video_metadata(context) },
        "global_start_time": zero,
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "Tracks",
            "children": [
                track("Video", "Video", video),
                track("Audio", "Audio", audio),
            ],
            "effects": [],
            "markers": [],
            "metadata": {},
            "source_range": Value::Null,
        },
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&timeline)?))
}

fn rational_time(value: u64, rate: f64) -> Value {
    json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "value": value,
        "rate": rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rational::FrameRate;
    use std::path::Path;

    #[test]
    fn emits_timeline_with_two_tracks_and_media_references() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let json_text = export(&context).unwrap();
        let value: Value = serde_json::from_str(&json_text).unwrap();
        assert_eq!(value["OTIO_SCHEMA"], "Timeline.1");
        let tracks = value["tracks"]["children"].as_array().unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0]["name"], "Video");
        assert_eq!(tracks[1]["name"], "Audio");
        assert_eq!(tracks[0]["children"].as_array().unwrap().len(), 3);
        // Official Clip.2 schema (OTIO serialized-data docs) uses the keyed
        // `media_references` map; `media_reference` was the Clip.1 field.
        assert_eq!(
            tracks[0]["children"][0]["media_references"]["DEFAULT_MEDIA"]["target_url"],
            "file:///media/input.mp4"
        );
        assert_eq!(
            tracks[0]["children"][1]["source_range"]["start_time"]["value"],
            90
        );
    }

    #[test]
    fn metadata_carries_probed_video_properties() {
        use crate::export::VideoInfo;

        let analysis = crate::export::tests::fixture();
        let video = VideoInfo {
            width: 3840,
            height: 2160,
            color_space: Some("bt2020nc".to_owned()),
            color_primaries: None,
            color_transfer: None,
            field_order: Some("tt".to_owned()),
        };
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::NTSC_2997,
            true,
        )
        .with_video(video);
        let value: Value = serde_json::from_str(&export(&context).unwrap()).unwrap();
        let metadata = &value["metadata"]["jumpcut"];
        assert_eq!(metadata["width"], 3840);
        assert_eq!(metadata["height"], 2160);
        assert_eq!(metadata["colorSpace"], "1-1-1 (Rec. 2020)");
        assert_eq!(metadata["scan"], "i");
    }
}

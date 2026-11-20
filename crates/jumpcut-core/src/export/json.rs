//! Machine-readable jumpcut timeline JSON exporter.
//!
//! Unlike the analysis JSON (which carries every loudness frame), this format
//! describes only the exported edit decision: source, exact rational durations,
//! frame rate and every edit with its source and record positions.

use serde_json::{Value, json};

use crate::analysis::{Analysis, Edit};
use crate::export::{ExportContext, ExportError, rational_seconds};
use crate::rational::Rational;

/// Renders the timeline JSON for `context`.
///
/// # Errors
///
/// Returns [`ExportError::Json`] if serialisation fails.
pub fn export(context: &ExportContext<'_>) -> Result<String, ExportError> {
    let mut cursor = Rational::ZERO;
    let mut edits = Vec::new();
    for edit in &context.analysis.plan.edits {
        let span = edit.span();
        let source_duration = span.duration();
        let mut item = json!({
            "kind": match edit {
                Edit::Keep { .. } => "keep",
                Edit::Cut { .. } => "cut",
                Edit::SpeedUp { .. } => "speed_up",
            },
            "source_start": rational_seconds(span.start),
            "source_end": rational_seconds(span.end),
            "source_duration": rational_seconds(source_duration),
        });
        if let Edit::SpeedUp { speed, .. } = edit {
            item["speed"] = json!(speed);
        }
        if let Some(recorded) = recorded_duration(edit, source_duration) {
            item["record_start"] = json!(rational_seconds(cursor));
            item["record_end"] = json!(rational_seconds(cursor + recorded));
            item["record_duration"] = json!(rational_seconds(recorded));
            cursor += recorded;
        }
        edits.push(item);
    }

    let document = json!({
        "version": 1,
        "source": context.source.to_string_lossy(),
        "source_duration": rational_seconds(context.analysis.plan.source_duration),
        "frame_rate": context.frame_rate.to_string(),
        "drop_frame": context.drop_frame,
        "threshold_dbfs": context.analysis.thresholds.base_dbfs,
        "output_duration": rational_seconds(context.output_duration()),
        "removed_duration": rational_seconds(context.analysis.removed_duration()),
        "edits": edits,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&document)?))
}

/// Builds a compact JSON summary of an analysis for `jumpcut analyze --json
/// --compact`.
///
/// The full analysis JSON carries every measured frame, which is huge for
/// multi-hour inputs. This variant keeps only exact rational durations,
/// thresholds and retained segments.
#[must_use]
pub fn compact_analysis(analysis: &Analysis) -> Value {
    let mut cursor = Rational::ZERO;
    let mut segments = Vec::new();
    for edit in &analysis.plan.edits {
        let span = edit.span();
        let source_duration = span.duration();
        let Some(recorded) = recorded_duration(edit, source_duration) else {
            continue;
        };
        let mut segment = json!({
            "source_start": rational_seconds(span.start),
            "source_end": rational_seconds(span.end),
            "record_start": rational_seconds(cursor),
            "record_end": rational_seconds(cursor + recorded),
            "record_duration": rational_seconds(recorded),
        });
        if let Edit::SpeedUp { speed, .. } = edit {
            segment["speed"] = json!(speed);
        }
        cursor += recorded;
        segments.push(segment);
    }
    json!({
        "version": 1,
        "sample_rate": analysis.sample_rate,
        "source_duration": rational_seconds(analysis.plan.source_duration),
        "output_duration": rational_seconds(analysis.output_duration()),
        "removed_duration": rational_seconds(analysis.removed_duration()),
        "thresholds": {
            "base_dbfs": analysis.thresholds.base_dbfs,
            "silence_dbfs": analysis.thresholds.silence_dbfs,
            "speech_dbfs": analysis.thresholds.speech_dbfs,
            "adaptive": analysis.thresholds.adaptive,
        },
        "segments": segments,
    })
}

fn recorded_duration(edit: &Edit, source_duration: Rational) -> Option<Rational> {
    match edit {
        Edit::Cut { .. } => None,
        Edit::Keep { .. } => Some(source_duration),
        Edit::SpeedUp { speed, .. } => {
            Some(source_duration / Rational::parse(&format!("{speed}")).unwrap_or(Rational::ONE))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    use crate::export::ExportFormat;
    use crate::rational::FrameRate;
    use std::path::Path;

    #[test]
    fn compact_analysis_omits_frames_and_keeps_exact_decisions() {
        let analysis = crate::export::tests::fixture();
        let value = compact_analysis(&analysis);
        assert!(
            value.get("frames").is_none(),
            "compact mode must drop frames"
        );
        assert_eq!(value["sample_rate"], 48_000);
        assert_eq!(value["source_duration"], "7/1s");
        assert_eq!(value["output_duration"], "5/1s");
        assert_eq!(value["removed_duration"], "2/1s");
        let segments = value["segments"].as_array().unwrap();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0]["source_start"], "0/1s");
        assert_eq!(segments[1]["record_start"], "2/1s");
        assert_eq!(segments[2]["source_end"], "7/1s");
        let full = serde_json::to_string(&analysis).unwrap();
        let compact = serde_json::to_string(&value).unwrap();
        assert!(
            compact.len() * 4 < full.len(),
            "compact JSON should be far smaller"
        );
    }

    #[test]
    fn emits_exact_durations_and_every_edit() {
        let analysis = crate::export::tests::fixture();
        let context = ExportContext::with_timecode(
            Path::new("/media/input.mp4"),
            &analysis,
            FrameRate::FPS_30,
            false,
        );
        let text = export(&context).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["source_duration"], "7/1s");
        assert_eq!(value["output_duration"], "5/1s");
        assert_eq!(value["removed_duration"], "2/1s");
        assert_eq!(value["edits"].as_array().unwrap().len(), 5);
        assert_eq!(value["edits"][0]["kind"], "keep");
        assert_eq!(value["edits"][1]["kind"], "cut");
        assert_eq!(value["edits"][4]["record_end"], "5/1s");
        assert_eq!(
            crate::export::export(&context, ExportFormat::Json).unwrap(),
            text
        );
    }
}

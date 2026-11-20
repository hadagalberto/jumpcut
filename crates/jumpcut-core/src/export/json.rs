//! Machine-readable jumpcut timeline JSON exporter.
//!
//! Unlike the analysis JSON (which carries every loudness frame), this format
//! describes only the exported edit decision: source, exact rational durations,
//! frame rate and every edit with its source and record positions.

use serde_json::json;

use crate::analysis::Edit;
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

//! Discovery of the external `FFmpeg` command-line tools.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Locates an executable on `PATH` (or an explicit path) without spawning it.
///
/// This lets the decoder and probe report a clear, actionable error before
/// ffmpeg is invoked, instead of a bare "No such file or directory".
pub fn find(tool: &str) -> Option<PathBuf> {
    let direct = Path::new(tool);
    if direct.components().count() > 1 || direct.is_absolute() {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    let path_variable = env::var_os("PATH")?;
    for directory in env::split_paths(&path_variable) {
        for name in executable_names(tool) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether `tool` can be found on `PATH`.
pub fn is_available(tool: &str) -> bool {
    find(tool).is_some()
}

/// Human-readable guidance for a missing `FFmpeg` tool.
pub fn missing_message(tool: &str) -> String {
    format!(
        "`{tool}` was not found in PATH. Install FFmpeg (including ffprobe) \
         and make sure its bin directory is on PATH. See https://ffmpeg.org/download.html"
    )
}

#[cfg(windows)]
fn executable_names(tool: &str) -> Vec<OsString> {
    [".exe", ".bat", ".cmd", ""]
        .iter()
        .map(|extension| OsString::from(format!("{tool}{extension}")))
        .collect()
}

#[cfg(not(windows))]
fn executable_names(tool: &str) -> Vec<OsString> {
    vec![OsString::from(tool)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn finds_an_explicit_executable_path() {
        let directory = tempfile::tempdir().unwrap();
        let name = format!("jumpcut-tool-{}", std::process::id());
        let path = directory.path().join(name);
        std::fs::write(&path, b"not really an executable").unwrap();
        assert_eq!(find(path.to_str().unwrap()), Some(path.clone()));
        assert_eq!(
            find(path.to_str().unwrap()).unwrap().as_os_str(),
            path.as_os_str()
        );
    }

    #[test]
    fn missing_explicit_path_is_reported() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("definitely-not-here");
        assert_eq!(find(missing.to_str().unwrap()), None);
    }

    #[test]
    fn missing_message_names_the_tool() {
        let message = missing_message("ffmpeg");
        assert!(message.contains("ffmpeg"));
        assert!(message.contains("FFmpeg"));
        assert!(!is_available("jumpcut-tool-that-does-not-exist-12345"));
    }

    #[test]
    fn executable_names_cover_the_platform() {
        let names = executable_names("ffmpeg");
        assert!(
            names
                .iter()
                .any(|name| name == OsStr::new("ffmpeg") || name == OsStr::new("ffmpeg.exe"))
        );
    }
}

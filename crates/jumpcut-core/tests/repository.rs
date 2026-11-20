//! Regression tests for the repository configuration fixed in milestone 5.
//!
//! These guard the cross-platform formatting and documentation problems that
//! cannot be observed from the library API alone.

use std::path::{Path, PathBuf};

fn workspace_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(name)
}

fn read(name: &str) -> String {
    std::fs::read_to_string(workspace_file(name))
        .unwrap_or_else(|error| panic!("could not read {name}: {error}"))
}

#[test]
fn gitattributes_normalises_text_and_leaves_media_binary() {
    let attributes = read(".gitattributes");
    assert!(
        attributes
            .lines()
            .any(|line| line.trim() == "* text=auto eol=lf"),
        "text files must be normalised to LF on every platform"
    );
    for pattern in ["*.mp4", "*.mov", "*.wav", "*.png", "*.zip"] {
        assert!(
            attributes
                .lines()
                .any(|line| line.starts_with(pattern) && line.contains("binary")),
            "{pattern} must be treated as binary"
        );
    }
}

#[test]
fn ci_uses_per_os_ffmpeg_and_the_release_test_gate() {
    let ci = read(".github/workflows/ci.yml");
    assert!(ci.contains("os: [ubuntu-latest, windows-latest, macos-latest]"));
    for os in ["Linux", "macOS", "Windows"] {
        assert!(
            ci.contains(&format!("Install ffmpeg ({os})")),
            "missing {os} ffmpeg install step"
        );
    }
    assert!(ci.contains("cargo fmt --all -- --check"));
    assert!(ci.contains("cargo clippy --workspace --all-targets -- -D warnings"));
    assert!(ci.contains("cargo test --workspace --release"));
    assert!(!ci.contains("RUSTFLAGS: -Dwarnings"));
}

#[test]
fn readme_examples_use_commands_that_actually_run() {
    let english = read("README.md");
    let portuguese = read("README.pt-BR.md");
    for text in [&english, &portuguese] {
        assert!(text.contains("-c aac podcast.m4a"));
        assert!(!text.contains("-c copy podcast.m4a"));
        assert!(text.contains("FFVideoFormatRateUndefined"));
    }
    assert!(!english.contains("force them on other NTSC rates"));
    assert!(!portuguese.contains("force-os em outras taxas NTSC"));
}

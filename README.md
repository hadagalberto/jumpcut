# jumpcut

A fast single-binary Rust CLI that removes silences and dead air from videos
and podcasts. It can render a file with ffmpeg or write an edit timeline for
DaVinci Resolve, Premiere, Final Cut, EDL, OTIO and ffmpeg concat lists.

Licensed under the [MIT License](LICENSE). Made by Hadagalberto Junior.

## Status

Milestones 1 (foundation), 2 (analysis) and 3 (outputs) are implemented.
Rendering, preview and all timeline exporters are available.

## Workspace

| Crate | Purpose |
| --- | --- |
| `crates/jumpcut-core` | Exact rational timing, timecode, ffprobe metadata, streaming PCM decode, silence analysis and edit planning |
| `crates/jumpcut` | clap-based command-line interface |

## Requirements

- Rust stable (edition 2024; toolchain 1.85 or newer)
- ffmpeg and ffprobe on `PATH` (only needed for media operations)

If ffmpeg is missing, jumpcut prints an installation hint instead of failing
with an opaque OS error.

## Build and test

```sh
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs those checks on Ubuntu, Windows and macOS with ffmpeg installed.

## CLI

```sh
# Show exact media metadata from ffprobe.
cargo run -p jumpcut -- probe input.mp4
cargo run -p jumpcut -- probe input.mp4 --json

# Format frame 1800 as 29.97 drop-frame timecode -> 00:01:00;02
cargo run -p jumpcut -- timecode 1800 --rate 29.97 --drop-frame

# Preview the silence-removal plan (no rendering).
cargo run -p jumpcut -- analyze input.mp4 --threshold=-35dB
cargo run -p jumpcut -- analyze input.mp4 --auto --margin 0.08s,0.15s
cargo run -p jumpcut -- analyze input.mp4 --silent-speed 4 --json

# Render with ffmpeg (default: libx264 -crf 18, aac 192k).
cargo run -p jumpcut -- input.mp4 -o cut.mp4
cargo run -p jumpcut -- input.mp4 -o cut.mp4 --hw-encoder h264_nvenc

# Stream-copy on keyframes (warns about imprecision).
cargo run -p jumpcut -- input.mp4 -o cut.mp4 --copy

# Print a terminal summary with a loudness sparkline, without rendering.
cargo run -p jumpcut -- input.mp4 --preview

# Write an edit timeline to stdout, or to a file with -o.
cargo run -p jumpcut -- input.mp4 --export fcpxml -o cut.fcpxml
cargo run -p jumpcut -- input.mp4 --export premiere
cargo run -p jumpcut -- input.mp4 --export edl --drop-frame
cargo run -p jumpcut -- input.mp4 --export otio
cargo run -p jumpcut -- input.mp4 --export concat
cargo run -p jumpcut -- input.mp4 --export json
```

Negative decibel values must use `--threshold=-35dB` so clap does not read the
minus sign as a new flag.

## Library highlights

- `Rational`: reduced `i128` fractions; `30000/1001` stays exact over hours.
- `FrameRate`: integer and NTSC rates, with `29.97`-style aliases mapped to
  their exact rational values.
- `Timecode`: non-drop and drop-frame SMPTE labels, with round-trip parsing.
- `probe_file`: ffprobe JSON decoded into exact durations and frame rates.
- `PcmDecoder`: streams `f32le` samples from ffmpeg through a pipe in bounded
  chunks; stderr is drained concurrently and checked on exit.
- `analysis`: streaming RMS/peak frames, absolute or documented adaptive
  threshold, hysteresis, margins, gap merging, exact frame snapping and
  silent-speed planning (`--auto` uses the 10th/90th percentile midpoint).
- `render`: chooses a `filter_complex` graph for small plans (and any
  silent-speed retime) or a concat-demuxer join for large ones; supports
  `--copy` with a keyframe-imprecision warning and encoder passthrough.
- `export`: FCPXML 1.10, Premiere xmeml v4, CMX3600 EDL (drop-frame), OTIO JSON,
  ffmpeg concat lists and machine-readable timeline JSON.
- `preview`: terminal summary with an ASCII loudness sparkline.

See [README.pt-BR.md](README.pt-BR.md) for Portuguese documentation,
[CHANGELOG.md](CHANGELOG.md) for release notes and
[CONTRIBUTING.md](CONTRIBUTING.md) for development rules.

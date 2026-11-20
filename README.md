# jumpcut

A fast single-binary Rust CLI that removes silences and dead air from videos
and podcasts. It can render a file with ffmpeg or write an edit timeline for
DaVinci Resolve, Premiere, Final Cut, EDL, OTIO and ffmpeg concat lists.

Licensed under the [MIT License](LICENSE). Made by Hadagalberto Junior.

## Status

Milestone 1 (foundation) is implemented. The full silence-removal pipeline and
the timeline exporters are planned for later milestones.

## Workspace

| Crate | Purpose |
| --- | --- |
| `crates/jumpcut-core` | Exact rational timing, timecode, ffprobe metadata, streaming PCM decode |
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

## CLI (milestone 1)

```sh
# Show exact media metadata from ffprobe.
cargo run -p jumpcut -- probe input.mp4
cargo run -p jumpcut -- probe input.mp4 --json

# Format frame 1800 as 29.97 drop-frame timecode -> 00:01:00;02
cargo run -p jumpcut -- timecode 1800 --rate 29.97 --drop-frame
```

## Library highlights

- `Rational`: reduced `i128` fractions; `30000/1001` stays exact over hours.
- `FrameRate`: integer and NTSC rates, with `29.97`-style aliases mapped to
  their exact rational values.
- `Timecode`: non-drop and drop-frame SMPTE labels, with round-trip parsing.
- `probe_file`: ffprobe JSON decoded into exact durations and frame rates.
- `PcmDecoder`: streams `f32le` samples from ffmpeg through a pipe in bounded
  chunks; stderr is drained concurrently and checked on exit.

See [README.pt-BR.md](README.pt-BR.md) for Portuguese documentation,
[CHANGELOG.md](CHANGELOG.md) for release notes and
[CONTRIBUTING.md](CONTRIBUTING.md) for development rules.

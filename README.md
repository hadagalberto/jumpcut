# jumpcut

A fast single-binary Rust CLI that removes silences and dead air from videos
and podcasts. It can render a file with ffmpeg or write an edit timeline for
DaVinci Resolve, Premiere, Final Cut, EDL, OTIO and ffmpeg concat lists.

Licensed under the [MIT License](LICENSE). Made by Hadagalberto Junior.

## Status

All four milestones are implemented: exact timing foundation, streaming silence
analysis, ffmpeg rendering plus six timeline exporters, and documentation with
criterion benchmarks. The build is checked with `cargo fmt --check`,
`cargo clippy -- -D warnings` and `cargo test --release` on Linux, Windows and
macOS.

## Installation

### Prebuilt binary

Tagged releases publish archives for Linux (`x86_64-unknown-linux-gnu`), Windows
(`x86_64-pc-windows-msvc`) and macOS (`x86_64-apple-darwin` and
`aarch64-apple-darwin`) on the
[GitHub Releases page](https://github.com/hadagalberto/jumpcut/releases).
Download the archive for your platform, unpack it and put the `jumpcut`
executable somewhere on your `PATH`. Verify with:

```sh
jumpcut --version
```

### From source

Requires a Rust toolchain of **1.85 or newer** (the workspace uses edition
2024):

```sh
git clone https://github.com/hadagalberto/jumpcut.git
cd jumpcut
cargo install --path crates/jumpcut --locked
```

or build a single optimized binary without installing:

```sh
cargo build --release -p jumpcut
# binary: target/release/jumpcut
```

### ffmpeg / ffprobe

Media operations shell out to **ffmpeg** and **ffprobe**; no FFI is involved.
Install them and make sure their `bin` directory is on your `PATH`:

```sh
# Windows (PowerShell)
winget install Gyan.FFmpeg          # or: choco install ffmpeg

# macOS
brew install ffmpeg

# Debian / Ubuntu
sudo apt-get update && sudo apt-get install ffmpeg

# Fedora (RPM Fusion)
sudo dnf install ffmpeg
```

Check with `ffmpeg -version` and `ffprobe -version`. If a tool is missing,
jumpcut prints an installation hint instead of an opaque OS error. Analysis of
pure PCM and all timing/timecode helpers do not need ffmpeg.

## Quick start

```sh
# Inspect the media (exact rational duration and frame rate).
jumpcut probe input.mp4

# See what would be cut, with a loudness sparkline — no rendering.
jumpcut input.mp4 --preview

# Render a frame-accurate tight cut (libx264 -crf 18, aac 192k).
jumpcut input.mp4 -o cut.mp4

# Or hand the edit decision to your NLE instead of re-encoding.
jumpcut input.mp4 --export fcpxml -o cut.fcpxml
```

## Usage recipes

### Livestreams and long recordings

For two-hour streams, let the adaptive threshold follow the room noise and
keep a little air around speech so breaths are not chopped:

```sh
# Preview the adaptive plan first.
jumpcut live.mp4 --preview --auto --min-silence 0.8s --margin 0.15s,0.25s

# Render frame-accurately (add a hardware encoder if you have one).
jumpcut live.mp4 -o live-cut.mp4 --auto --min-silence 0.8s \
  --margin 0.15s,0.25s --hw-encoder h264_nvenc

# Or export the cut decision for your editor — no re-encode needed.
jumpcut live.mp4 --auto --min-silence 0.8s --export fcpxml -o live-cut.fcpxml

# Instead of deleting long pauses, play them back at 4x.
jumpcut live.mp4 -o live-cut.mp4 --auto --silent-speed 4
```

Frame boundaries snap exactly to the source frame rate read by ffprobe, so
NTSC rates (`30000/1001`, `24000/1001`, `60000/1001`) do not drift over a long
stream.

### Podcasts and multi-microphone audio

For spoken-word audio, an absolute threshold plus asymmetric margins keeps
natural pauses without sounding clipped:

```sh
jumpcut episode.wav --threshold=-35dB --min-silence 0.6s \
  --margin 0.15s,0.30s --preview
jumpcut episode.wav -o episode-cut.wav --threshold=-35dB --min-silence 0.6s \
  --margin 0.15s,0.30s
```

With separate per-speaker tracks, mux them into one multi-stream file and let
jumpcut combine streams by maximum loudness. A quiet guest then stays in even
when the host is silent:

```sh
ffmpeg -i host.wav -i guest.wav -map 0:a -map 1:a -c copy podcast.m4a
jumpcut podcast.m4a --combine-streams --auto --min-silence 0.6s --preview
jumpcut podcast.m4a -o podcast-cut.m4a --combine-streams --auto --min-silence 0.6s
```

Use `--audio-stream 1` instead to analyse one specific audio stream.

### DaVinci Resolve

```sh
jumpcut interview.mp4 --auto --export fcpxml -o resolve.fcpxml
```

In Resolve, choose **File → Import → Timeline…** (or drag the `.fcpxml` onto
the timeline). The XML uses FCPXML 1.10 with a spine of `asset-clip`s and exact
`N/Ds` rational durations. Keep the original media at the same path — the
timeline references it by absolute `file://` URL.

### Adobe Premiere Pro

```sh
jumpcut interview.mp4 --auto --export premiere -o premiere.xml
```

Import the XML with **File → Import** (or drag it into the Project panel).
jumpcut writes an xmeml v4 sequence with video and audio tracks of `clipitem`s.
For 29.97 material drop-frame labels are emitted automatically; force them on
other NTSC rates with `--drop-frame`.

### Final Cut Pro

```sh
jumpcut interview.mp4 --auto --export fcpxml -o finalcut.fcpxml
```

In Final Cut Pro use **File → Import → XML…**. The document declares a format
and asset resource and a project whose spine holds one `asset-clip` per kept
span; sped-up spans carry a `<timeMap>` retime. The original media must remain
reachable at the referenced path.

### Other exchanges

```sh
# CMX3600 EDL (drop-frame aware, for tape/online workflows).
jumpcut input.mp4 --export edl -o cut.edl

# OpenTimelineIO JSON (video + audio tracks, LinearTimeWarp for retimes).
jumpcut input.mp4 --export otio -o cut.otio.json

# ffmpeg concat demuxer list referencing the original media.
jumpcut input.mp4 --export concat -o cut.ffconcat

# Machine-readable timeline (exact rational durations for every edit).
jumpcut input.mp4 --export json -o cut.json

# Machine-readable full analysis (per-frame loudness, thresholds, plan).
jumpcut input.mp4 --auto --json > analysis.json
```

## Tuning

Preview first, adjust, then render: `jumpcut INPUT --preview [knobs]` is cheap
and shows the segment count, removed time/percentage and a loudness sparkline.

| Flag | Default | What it does / when to change it |
| --- | --- | --- |
| `--threshold=-35dB` | `-35 dBFS` | Absolute silence level. Good for consistent recordings. Negative values need the `=` form so clap does not read `-` as a flag. |
| `--auto` | off | Adaptive threshold: noise floor = 10th loudness percentile, speech = 90th, threshold at their midpoint on the dB scale (documented in `analysis::threshold`). Use for varying rooms or live audio. |
| `--hysteresis 3` | `3 dB` | Separate enter/exit levels. Widen it if cuts flap near the threshold; narrow it to catch borderline silences. |
| `--min-silence 0.5s` | `0.5 s` | Ignore silences shorter than this (breaths, musical gaps). Raise for podcasts, lower for rapid speech. |
| `--min-clip 0.3s` | `0.3 s` | Absorb kept fragments shorter than this into the neighbouring cut. Raise to avoid staccato edits. |
| `--margin 0.1s` | `0.1 s` | Silence retained on both sides of speech. Use `0.08s,0.15s` for different before/after values; raise if cuts sound abrupt. |
| `--merge-gap 0.2s` | `0 s` | Join kept spans separated by gaps shorter than this. |
| `--silent-speed 4` | off | Speed silences up instead of cutting them. Speeds above 4 are chained into multiple `atempo` filters. |
| `--fps 30000/1001` | from ffprobe | Snap cut boundaries exactly to a frame grid. Rarely needed — the source rate is detected automatically. |
| `--copy` | off | Stream-copy the original codecs. Fast, but cuts snap to keyframes; jumpcut warns about the imprecision. |
| `--hw-encoder NAME` | `libx264` | Pass an encoder name through to ffmpeg (e.g. `h264_nvenc`, `h264_qsv`). |

Other CLI helpers:

```sh
# Show exact media metadata as JSON.
jumpcut probe input.mp4 --json

# Format frame 1800 at 29.97 drop-frame -> 00:01:00;02
jumpcut timecode 1800 --rate 29.97 --drop-frame

# Tune the measurement grid (defaults: 30 ms window, 10 ms hop).
jumpcut input.mp4 --window 40ms --hop 10ms --preview
```

## Workspace

| Crate | Purpose |
| --- | --- |
| `crates/jumpcut-core` | Exact rational timing, timecode, ffprobe metadata, streaming PCM decode, silence analysis, edit planning, ffmpeg rendering, timeline exporters and preview |
| `crates/jumpcut` | clap-based command-line interface |

## Build, test and benchmark

```sh
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
cargo bench -p jumpcut-core --bench analysis
```

Criterion benchmarks cover streaming frame measurement (one minute and two
hours), the full analysis pipeline, exact NTSC snapping and drop-frame timecode
round-trips. CI repeats the build, format, clippy and test checks on Ubuntu,
Windows and macOS with ffmpeg installed.

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
  silent-speed planning.
- `render`: chooses a `filter_complex` graph for small plans (and any
  silent-speed retime) or a concat-demuxer join for large ones; supports
  `--copy` with a keyframe-imprecision warning and encoder passthrough.
- `export`: FCPXML 1.10, Premiere xmeml v4, CMX3600 EDL (drop-frame), OTIO JSON,
  ffmpeg concat lists and machine-readable timeline JSON.
- `preview`: terminal summary with an ASCII loudness sparkline.

See [README.pt-BR.md](README.pt-BR.md) for Portuguese documentation,
[CHANGELOG.md](CHANGELOG.md) for release notes and
[CONTRIBUTING.md](CONTRIBUTING.md) for development rules.

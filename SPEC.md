# jumpcut — SPEC

**jumpcut**: a fast single-binary Rust CLI that removes silences/dead air from videos and podcasts, producing either a rendered file (via ffmpeg)
or an edit timeline for NLEs (DaVinci Resolve / Premiere / Final Cut via FCPXML 1.10, Premiere XML (xmeml v4), EDL CMX3600, OTIO JSON, plus ffmpeg concat lists).
For creators: edit a 2-hour live into a tight cut in seconds, and keep full control in your editor. Open source, MIT, "Hadagalberto Junior".
NOT a toy: no stubs, no `todo!()`, no `unimplemented!()`, no placeholder or ignored tests.

## Stack
Rust stable (edition 2024 if supported by the installed toolchain, else 2021), workspace: `jumpcut-core` (lib: analysis, segment planning, timeline export) and `jumpcut` (CLI, clap derive).
Audio decoding by invoking **ffmpeg/ffprobe** as subprocesses (`ffmpeg -i in -ac 1 -ar 16000 -f f32le -`), streaming PCM through a pipe — no FFI. Detect missing ffmpeg with a clear error.
`cargo clippy -- -D warnings` (pedantic where reasonable), `cargo fmt --check`, tests with `cargo test`, property tests with `proptest`, snapshot tests with `insta`.
GitHub Actions CI (ubuntu, windows, macos; installs ffmpeg) + release workflow building binaries with `cargo-dist` style matrix (do not run releases).
README.md + README.pt-BR.md, CHANGELOG, CONTRIBUTING, LICENSE (MIT).

## Analysis
- Frame-based RMS/peak in dBFS over configurable windows (default 30 ms hop 10 ms), streaming (constant memory for multi-hour inputs).
- Silence detection: threshold modes `--threshold -35dB` (absolute) or `--auto` (adaptive: estimate noise floor from the 10th percentile of frame loudness and speech level from the 90th, threshold between them, documented formula).
- Hysteresis (separate enter/exit thresholds), minimum silence duration (`--min-silence 0.5s`), minimum kept segment (`--min-clip 0.3s`), padding/margins before/after speech (`--margin 0.1s` or `--margin 0.08s,0.15s`),
  merge segments separated by short gaps, optional **speed-up silences** instead of cutting (`--silent-speed 4`) with audio atempo chaining.
- Optional multi-track: analyze a specific audio stream (`--audio-stream 1`) or combine streams (max).
- Frame-accurate snapping to the video frame rate (read via ffprobe, handle NTSC rates 30000/1001, 24000/1001, 60000/1001 exactly using rational arithmetic — no float drift on long timelines).

## Outputs
- `jumpcut in.mp4 -o out.mp4`: render via ffmpeg using `select/aselect` filter scripts or concat demuxer of trimmed segments; choose strategy automatically by segment count;
  preserve codecs when `--copy` is possible on keyframes (warn about imprecision), else re-encode with sane defaults (`libx264 -crf 18`, `aac 192k`), hardware encoder flag passthrough.
- `--export fcpxml|premiere|edl|otio|concat|json` writes timelines referencing the original media with exact rational timecodes. FCPXML must validate structurally against the FCPXML 1.10 rules (asset, format, spine with asset-clips, offsets/starts/durations as rational seconds `N/Ds`).
  EDL CMX3600 with drop-frame handling for 29.97. Premiere xmeml with sequence + clipitems.
- `--preview` prints a terminal summary (segments, removed duration, percentage, ASCII loudness sparkline) without rendering; `--json` emits machine-readable analysis.

## Tests
- Synthetic audio fixtures generated in tests (sine tones + silence + white noise at known dB, written via ffmpeg lavfi when available, and pure-Rust generated PCM for core tests not needing ffmpeg) with exact expected segments.
- Property tests: segments are sorted, non-overlapping, within media duration, respect min-clip/margins; total kept + removed = duration.
- Snapshot tests for every exporter (insta), timecode math unit tests incl. NTSC drop-frame boundaries (e.g. 00:00:59;29 → 00:01:00;02).
- End-to-end tests (skipped gracefully with a printed reason only if ffmpeg is absent — CI always has it): render a generated video and assert output duration via ffprobe within one frame.
- Performance: analysing 2 hours of 16 kHz mono synthetic PCM < 3 s in release mode (benchmark with criterion; test asserts a generous bound).

## Hard constraints
- Never run `git push`, `gh`, `cargo publish`, or publish anything. Commit locally after each milestone. Stay inside this repository.

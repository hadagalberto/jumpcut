# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
semantic versioning.

## [Unreleased]

### Added (milestone 3 — outputs)

- ffmpeg rendering with automatic strategy selection: a `filter_complex` graph
  for small plans and silent-speed retimes, a concat-demuxer join for large
  plans. Re-encoding defaults to `libx264 -crf 18` / `aac 192k`; `--copy`
  stream-copies codecs with a keyframe-imprecision warning; `--hw-encoder`
  passes an encoder name through to ffmpeg.
- Exact `atempo` chaining for `--silent-speed`, with a retimed video frame rate
  so output durations stay accurate.
- Timeline exporters: FCPXML 1.10 (asset/format/spine, `N/Ds` rational seconds),
  Premiere xmeml v4 (sequence + clipitems), CMX3600 EDL with drop-frame labels,
  OTIO JSON, ffmpeg concat lists and machine-readable timeline JSON.
- `--preview` terminal summary with segment counts, removed time/percentage and
  an ASCII loudness sparkline; `--export` writes timelines to stdout or a file.
- `insta` snapshots for all six exporters plus the preview, and end-to-end
  render tests that generate media and assert output duration via ffprobe.

### Added (milestone 2 — analysis)

- Streaming frame analysis: configurable RMS/peak dBFS windows and hops
  (defaults 30 ms / 10 ms) with bounded PCM memory.
- Absolute (`--threshold`) and adaptive (`--auto`) thresholds; the adaptive
  formula uses the 10th/90th loudness percentiles and is documented in the
  `analysis::threshold` module.
- Two-level hysteresis, `--min-silence`, `--min-clip`, one- or two-sided
  `--margin`, short-gap merging and exact frame snapping (including NTSC rates).
- `--silent-speed` planning with `atempo` factor chaining for speeds above 4x.
- Single-stream and max-combined multi-stream analysis via `--audio-stream` /
  `--combine-streams`.
- `analyze` subcommand with terminal preview (loudness sparkline) and `--json`.
- Pure-Rust synthetic PCM tests with exact segments, proptest invariants and a
  two-hour release-mode performance test.

## [0.1.0] - 2026-11-20

### Added

- Cargo workspace with `jumpcut-core` (library) and `jumpcut` (clap CLI).
- `rustfmt.toml`, `clippy.toml` and CI for Ubuntu, Windows and macOS with
  ffmpeg installation.
- Release workflow skeleton (never run from local development).
- MIT LICENSE, README (EN/PT-BR), CHANGELOG and CONTRIBUTING.
- Exact `Rational` / `FrameRate` arithmetic with NTSC rates and frame snapping.
- SMPTE timecode formatting/parsing including drop-frame boundaries.
- `ffprobe` JSON metadata reading with exact rational durations/rates.
- Streaming ffmpeg `f32le` PCM decoding over a pipe with clear missing-tool
  errors and checked exit status.
- Unit and property tests for timing, timecode, probing and decoding.

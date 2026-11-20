# Changelog

All notable changes to this project are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
semantic versioning.

## [Unreleased]

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

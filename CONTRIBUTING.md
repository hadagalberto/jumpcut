# Contributing

## Ground rules

- No stubs, `todo!()`, `unimplemented!()`, placeholder or ignored tests.
- Tests must assert real behaviour with known-valid and known-invalid vectors.
- Never weaken or delete a failing test to make a build pass: fix the root
  cause.
- Never push, publish, run `cargo publish`, `gh` or create releases from local
  work. Commit locally only.

## Development loop

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
RUSTFLAGS=-Dwarnings cargo build --workspace --all-targets
cargo test --workspace
```

All four must pass before committing. The CI matrix repeats them on Ubuntu,
Windows and macOS with ffmpeg installed.

## Tests that need ffmpeg

Tests which generate audio with ffmpeg skip themselves with a printed reason
when ffmpeg/ffprobe are unavailable. CI always has them, so regressions cannot
hide behind a skip there. Core timing tests are pure Rust and always run.

## Commits

Use Conventional Commits (`feat:`, `fix:`, `docs:`, `chore:`, `test:`, `refactor:`).

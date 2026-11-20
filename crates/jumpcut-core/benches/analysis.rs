//! Criterion benchmarks for jumpcut's analysis hot path.
//!
//! These measure the work that dominates a real silence-removal run: streaming
//! frame measurement over a bounded PCM pipe, the full detect-and-plan
//! pipeline, exact NTSC snapping and SMPTE timecode round-trips. The two-hour
//! streaming case mirrors the release-mode performance bound asserted by
//! `analysis::tests::two_hour_performance_bound`.
//!
//! Run with `cargo bench -p jumpcut-core`, or a single group with
//! `cargo bench -p jumpcut-core --bench analysis -- streaming_frames`.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Throughput, criterion_group, criterion_main};
use jumpcut_core::{
    AnalysisConfig, FrameConfig, FrameRate, Margins, Rational, ThresholdSelection, Timecode,
    analyze_samples, frames_from_chunks,
};

const CHUNK_SAMPLES: usize = 8_192;
const SAMPLE_RATE: u32 = 16_000;
/// 118 chunks of 8192 samples at 16 kHz is just over one minute.
const MINUTE_TICKS: usize = 118;
/// 14,063 chunks of 8192 samples at 16 kHz is just over two hours.
const TWO_HOUR_TICKS: usize = 14_063;

fn analysis_config() -> AnalysisConfig {
    AnalysisConfig {
        frame: FrameConfig::default(),
        threshold: ThresholdSelection::Absolute(-35.0),
        hysteresis_db: 3.0,
        min_silence: Rational::new(1, 2).unwrap(),
        min_clip: Rational::new(3, 10).unwrap(),
        margins: Margins::default(),
        merge_gap: Rational::ZERO,
        silent_speed: None,
        frame_rate: Some(FrameRate::NTSC_2997),
    }
}

/// A small pool of alternating speech/silence chunks that is cycled to
/// synthesise arbitrarily long streams without allocating per iteration.
fn chunk_pool() -> Vec<Vec<f32>> {
    (0..8)
        .map(|index| {
            let amplitude = if index % 2 == 0 { 0.25 } else { 0.0 };
            vec![amplitude; CHUNK_SAMPLES]
        })
        .collect()
}

fn stream(pool: &[Vec<f32>], ticks: usize) -> impl Iterator<Item = &[f32]> {
    pool.iter().cycle().take(ticks).map(Vec::as_slice)
}

fn bench_streaming_frames(c: &mut criterion::Criterion) {
    let pool = chunk_pool();
    let config = FrameConfig::default();

    let mut minute = c.benchmark_group("streaming_frames/1min");
    minute.throughput(Throughput::Elements((MINUTE_TICKS * CHUNK_SAMPLES) as u64));
    minute.bench_function("16k_mono", |b| {
        b.iter(|| {
            let frames =
                frames_from_chunks(black_box(stream(&pool, MINUTE_TICKS)), SAMPLE_RATE, &config)
                    .unwrap();
            black_box(frames.len())
        })
    });
    minute.finish();

    // Two hours produces 720,000 frames. Keep the sample count small so the
    // benchmark stays usable in CI while still covering the full workload.
    let mut hours = c.benchmark_group("streaming_frames/2h");
    hours
        .sample_size(10)
        .measurement_time(Duration::from_secs(5))
        .throughput(Throughput::Elements(
            (TWO_HOUR_TICKS * CHUNK_SAMPLES) as u64,
        ));
    hours.bench_function("16k_mono", |b| {
        b.iter(|| {
            let frames = frames_from_chunks(
                black_box(stream(&pool, TWO_HOUR_TICKS)),
                SAMPLE_RATE,
                &config,
            )
            .unwrap();
            black_box(frames.len())
        })
    });
    hours.finish();
}

fn bench_full_pipeline(c: &mut criterion::Criterion) {
    let pool = chunk_pool();
    let samples: Vec<f32> = stream(&pool, MINUTE_TICKS).flatten().copied().collect();
    let config = analysis_config();
    let mut group = c.benchmark_group("analyze_samples");
    group.throughput(Throughput::Elements(samples.len() as u64));
    group.bench_function("1min_16k_mono", |b| {
        b.iter(|| {
            let analysis = analyze_samples(black_box(&samples), SAMPLE_RATE, &config).unwrap();
            black_box(analysis.plan.edit_count())
        })
    });
    group.finish();
}

fn bench_exact_snapping(c: &mut criterion::Criterion) {
    let rate = FrameRate::NTSC_2997;
    let times: Vec<Rational> = (0..10_000)
        .map(|frame| Rational::new(frame * 1_001, 30_000).unwrap())
        .collect();
    let mut group = c.benchmark_group("rational");
    group.throughput(Throughput::Elements(times.len() as u64));
    group.bench_function("snap_ntsc_2997", |b| {
        b.iter(|| {
            let mut checksum = 0_i128;
            for time in black_box(&times) {
                let snapped = time.snapped_to_frame(black_box(rate));
                checksum = checksum.wrapping_add(snapped.num());
            }
            black_box(checksum)
        })
    });
    group.finish();
}

fn bench_timecode(c: &mut criterion::Criterion) {
    let rate = FrameRate::NTSC_2997;
    let codes: Vec<Timecode> = (0..10_000)
        .map(|frames| Timecode::new(frames, rate, true).unwrap())
        .collect();
    let mut group = c.benchmark_group("timecode");
    group.throughput(Throughput::Elements(codes.len() as u64));
    group.bench_function("drop_frame_format_parse", |b| {
        b.iter(|| {
            black_box(&codes)
                .iter()
                .map(|code| {
                    Timecode::parse(&code.format(), rate, true)
                        .unwrap()
                        .frames()
                })
                .sum::<u64>()
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_streaming_frames,
    bench_full_pipeline,
    bench_exact_snapping,
    bench_timecode
);
criterion_main!(benches);

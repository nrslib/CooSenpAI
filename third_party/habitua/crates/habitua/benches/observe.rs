use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use habitua::{
    Config, FlyModel, NearestNeighborConfig, NearestNeighborModel, NumericDeviationConfig,
    NumericDeviationModel, Timestamp,
};

fn evaluate_and_commit(engine: &mut FlyModel, stream_id: &str, features: &[f32], timestamp: u64) {
    let evaluation = engine
        .evaluate(
            stream_id,
            black_box(features),
            Timestamp::from_millis(timestamp),
        )
        .expect("benchmark input is valid");
    black_box((
        evaluation.novelty_short,
        evaluation.novelty_long,
        evaluation.pattern_id,
    ));
    engine
        .commit(evaluation.update, 1.0)
        .expect("benchmark update is valid");
}

fn observe_128_dimensions(criterion: &mut Criterion) {
    let mut engine = FlyModel::fly_default();
    let features: Vec<f32> = (0..128)
        .map(|index| (index as f32 * 0.03125).sin())
        .collect();
    let mut timestamp = 0_u64;
    criterion.bench_function("evaluate_commit_128_dimensions", |bencher| {
        bencher.iter(|| {
            evaluate_and_commit(&mut engine, "benchmark", &features, timestamp);
            timestamp += 1;
        });
    });
}

fn observe_128_dimensions_with_drifting_input(criterion: &mut Criterion) {
    let mut engine = FlyModel::fly_default();
    let mut timestamp = 0_u64;
    criterion.bench_function("evaluate_commit_128_dimensions_drifting", |bencher| {
        bencher.iter(|| {
            let features: Vec<f32> = (0..128)
                .map(|index| {
                    let phase = index as f32 * 0.03125 + timestamp as f32 * 0.0001;
                    phase.sin()
                })
                .collect();
            evaluate_and_commit(&mut engine, "benchmark-drifting", &features, timestamp);
            timestamp += 1;
        });
    });
}

fn saturated_fly_input(criterion: &mut Criterion) {
    let config = Config {
        dimensions: 128,
        encoder_units: 512,
        k: 16,
        max_patterns: 64,
        ..Config::default()
    };
    let mut engine = FlyModel::with_config(config).expect("saturation configuration is valid");
    for timestamp in 0..64_u64 {
        let features: Vec<f32> = (0..128)
            .map(|index| ((index + timestamp as usize) as f32 * 0.013).sin())
            .collect();
        evaluate_and_commit(&mut engine, "saturated", &features, timestamp);
    }
    let mut timestamp = 64_u64;
    criterion.bench_function("evaluate_commit_saturated_lru", |bencher| {
        bencher.iter(|| {
            let features: Vec<f32> = (0..128)
                .map(|index| ((index + timestamp as usize) as f32 * 0.017).cos())
                .collect();
            evaluate_and_commit(&mut engine, "saturated", &features, timestamp);
            timestamp += 1;
        });
    });
}

fn multiple_streams(criterion: &mut Criterion) {
    let mut engine = FlyModel::fly_default();
    let features: Vec<f32> = (0..128)
        .map(|index| (index as f32 * 0.03125).sin())
        .collect();
    let mut timestamp = 0_u64;
    criterion.bench_function("evaluate_commit_64_streams", |bencher| {
        bencher.iter(|| {
            let stream = format!("stream-{}", timestamp % 64);
            evaluate_and_commit(&mut engine, &stream, &features, timestamp);
            timestamp += 1;
        });
    });
}

fn numeric_deviation(criterion: &mut Criterion) {
    let config = NumericDeviationConfig {
        minimum_samples: 1,
        ..NumericDeviationConfig::default()
    };
    let mut model = NumericDeviationModel::with_config(["latency_ms", "retry_count"], config)
        .expect("numeric configuration is valid");
    let mut timestamp = 0_u64;
    criterion.bench_function("numeric_deviation_two_values", |bencher| {
        bencher.iter(|| {
            let evaluation = model
                .evaluate(
                    "numeric",
                    black_box(&[50.0, 1.0]),
                    Timestamp::from_millis(timestamp),
                )
                .expect("benchmark input is valid");
            black_box(evaluation.diagnostics);
            model.commit(evaluation.update, 1.0).expect("valid update");
            timestamp += 1;
        });
    });
}

fn nearest_neighbor(criterion: &mut Criterion) {
    let config = NearestNeighborConfig {
        dimensions: 128,
        max_representatives: 256,
        ..NearestNeighborConfig::default()
    };
    let mut model = NearestNeighborModel::new(config).expect("nearest configuration is valid");
    let features: Vec<f32> = (0..128)
        .map(|index| (index as f32 * 0.03125).sin())
        .collect();
    let mut timestamp = 0_u64;
    criterion.bench_function("nearest_neighbor_128_dimensions", |bencher| {
        bencher.iter(|| {
            let evaluation = model
                .evaluate(
                    "nearest",
                    black_box(&features),
                    Timestamp::from_millis(timestamp),
                )
                .expect("benchmark input is valid");
            black_box(evaluation.diagnostics);
            model.commit(evaluation.update, 1.0).expect("valid update");
            timestamp += 1;
        });
    });
}

fn evaluate_commit_4096_dimensions_2048_units(criterion: &mut Criterion) {
    let config = Config {
        dimensions: 4_096,
        encoder_units: 2_048,
        k: 32,
        projection_density: 0.05,
        ..Config::default()
    };
    let mut engine = FlyModel::with_config(config).expect("large benchmark configuration is valid");
    let features: Vec<f32> = (0..4_096)
        .map(|index| (index as f32 * 0.001953125).sin())
        .collect();
    let mut timestamp = 0_u64;
    criterion.bench_function("evaluate_commit_4096_dimensions_2048_units", |bencher| {
        bencher.iter(|| {
            evaluate_and_commit(&mut engine, "benchmark-large", &features, timestamp);
            timestamp += 1;
        });
    });
}

criterion_group!(
    benches,
    observe_128_dimensions,
    observe_128_dimensions_with_drifting_input,
    saturated_fly_input,
    multiple_streams,
    numeric_deviation,
    nearest_neighbor,
    evaluate_commit_4096_dimensions_2048_units
);
criterion_main!(benches);

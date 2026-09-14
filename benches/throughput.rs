use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use metra::{BatchOptions, FileFormat, FileInfo, ParseLimits};

fn benchmark_jpeg() -> Vec<u8> {
    let comment = b"benchmark";
    let length = u16::try_from(comment.len() + 2).expect("benchmark comment fits in a segment");
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xFE];
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(comment);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

fn bench_stream_read(c: &mut Criterion) {
    let bytes = benchmark_jpeg();
    let mut group = c.benchmark_group("stream_read");
    group.throughput(Throughput::Bytes(bytes.len() as u64));
    group.bench_function("jpeg", |bench| {
        bench.iter(|| {
            let mut reader = Cursor::new(bytes.as_slice());
            let info = FileInfo::new(
                "benchmark.jpg".into(),
                bytes.len() as u64,
                FileFormat::Unknown,
            );
            black_box(metra::read_from(&mut reader, info).expect("benchmark JPEG should parse"));
        });
    });
    group.finish();
}

fn bench_local_corpus(c: &mut Criterion) {
    let root = std::env::var_os("METRA_BENCH_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus"));
    let mut files = Vec::new();
    collect_files(&root, &mut files).expect("benchmark corpus should be traversable");
    files.sort();
    if files.is_empty() {
        eprintln!("METRA_BENCH_CORPUS is empty; skipping corpus benchmark");
        return;
    }
    let total_bytes = files
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum();
    let mut group = c.benchmark_group("local_corpus");
    group.throughput(Throughput::Bytes(total_bytes));
    group.bench_function("read_all", |bench| {
        bench.iter(|| {
            let recognized = files
                .iter()
                .filter(|path| metra::read(path).is_ok())
                .count();
            black_box(recognized);
        });
    });
    for jobs in [1, 4] {
        group.bench_with_input(BenchmarkId::new("read_many", jobs), &jobs, |bench, jobs| {
            bench.iter(|| {
                let results = metra::read_many(
                    &files,
                    BatchOptions {
                        jobs: *jobs,
                        limits: ParseLimits::default(),
                    },
                );
                let recognized = results.iter().filter(|item| item.result.is_ok()).count();
                black_box(recognized);
            });
        });
    }
    group.bench_function("read_many_streaming_4", |bench| {
        bench.iter(|| {
            let mut recognized = 0_usize;
            metra::read_many_streaming(
                &files,
                BatchOptions {
                    jobs: 4,
                    limits: ParseLimits::default(),
                },
                |item| {
                    recognized += usize::from(item.result.is_ok());
                },
            );
            black_box(recognized);
        });
    });
    group.finish();
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        collect_files(&entry?.path(), files)?;
    }
    Ok(())
}

criterion_group!(benches, bench_stream_read, bench_local_corpus);
criterion_main!(benches);

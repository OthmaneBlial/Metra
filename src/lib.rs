//! Public library facade for Metra.
//!
//! The command-line application is deliberately a thin consumer of this API.
//! Format-specific parsing lives in `metra-formats`; stable data types and
//! safety limits live in `metra-core`.

use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

pub use metra_core::{
    CapabilityStatus, FileFormat, FileInfo, FormatCapabilities, Metadata, MetadataDiff, MetraError,
    ParseLimits, Result, Source, Tag, TagDefinition, TagDifference, TagValue, ValueType, Warning,
    format_capabilities, format_capabilities_all, tag_definition, tag_definitions,
};
pub use metra_formats::{
    DetectedFormat, FlacEdit, GifEdit, IsobmffEdit, JpegEdit, Mp3Edit, PngEdit, SvgEdit, TiffEdit,
    WavEdit, WebpEdit, detect_format, read_path_with_limits, read_reader, read_reader_with_limits,
    rewrite_flac, rewrite_flac_path, rewrite_flac_to_vec, rewrite_gif, rewrite_gif_path,
    rewrite_gif_to_vec, rewrite_isobmff, rewrite_isobmff_path, rewrite_isobmff_to_vec,
    rewrite_jpeg, rewrite_jpeg_path, rewrite_jpeg_to_vec, rewrite_mp3, rewrite_mp3_path,
    rewrite_mp3_to_vec, rewrite_png, rewrite_png_path, rewrite_png_to_vec, rewrite_svg,
    rewrite_svg_path, rewrite_svg_to_vec, rewrite_tiff, rewrite_tiff_path, rewrite_tiff_to_vec,
    rewrite_wav, rewrite_wav_path, rewrite_wav_to_vec, rewrite_webp, rewrite_webp_path,
    rewrite_webp_to_vec,
};

/// Read metadata from a path using the default defensive parser limits.
pub fn read(path: impl AsRef<Path>) -> Result<Metadata> {
    metra_formats::read_path(path)
}

/// Read metadata from a path with caller-selected resource limits.
pub fn read_with_limits(path: impl AsRef<Path>, limits: ParseLimits) -> Result<Metadata> {
    read_path_with_limits(path, limits)
}

/// Read metadata from a seekable stream using the default defensive limits.
pub fn read_from<R: Read + Seek>(reader: &mut R, file_info: FileInfo) -> Result<Metadata> {
    read_reader(reader, file_info)
}

/// Read metadata from a seekable stream with caller-selected limits.
pub fn read_from_with_limits<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    read_reader_with_limits(reader, file_info, limits)
}

/// Options shared by the public batch inspection helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchOptions {
    /// Maximum number of files inspected concurrently. Zero is treated as one.
    pub jobs: usize,
    /// Defensive parser limits applied to every file.
    pub limits: ParseLimits,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self {
            jobs: 1,
            limits: ParseLimits::default(),
        }
    }
}

/// One deterministic batch result, retaining the input path next to success
/// or structured read failure.
#[derive(Debug)]
pub struct BatchResult {
    pub path: PathBuf,
    pub result: Result<Metadata>,
}

/// Inspect independent paths with bounded worker concurrency.
///
/// Results always follow the order of `paths`, including when workers finish
/// out of order. The returned vector intentionally contains one result per
/// input, so callers that need bounded output should use
/// [`read_many_streaming`] instead.
pub fn read_many(paths: &[PathBuf], options: BatchOptions) -> Vec<BatchResult> {
    if paths.is_empty() {
        return Vec::new();
    }
    let worker_count = options.jobs.max(1).min(paths.len());
    if worker_count == 1 {
        return paths
            .iter()
            .cloned()
            .map(|path| BatchResult {
                result: read_with_limits(&path, options.limits),
                path,
            })
            .collect();
    }

    let shared_paths = Arc::new(paths.to_vec());
    let next_index = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    let mut slots = (0..paths.len())
        .map(|_| None)
        .collect::<Vec<Option<BatchResult>>>();

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let paths = Arc::clone(&shared_paths);
            let next_index = Arc::clone(&next_index);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index).cloned() else {
                        break;
                    };
                    let result = read_with_limits(&path, options.limits);
                    if sender.send((index, BatchResult { path, result })).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, result) in receiver {
            slots[index] = Some(result);
        }
    });

    slots
        .into_iter()
        .map(|slot| slot.expect("every scheduled path should produce a result"))
        .collect()
}

/// Inspect paths in deterministic order while emitting each result as soon as
/// all earlier paths are ready.
///
/// Parallel workers use a bounded synchronous channel. If an early path is
/// slow, later results apply backpressure instead of growing an unbounded
/// out-of-order buffer.
pub fn read_many_streaming<F>(paths: &[PathBuf], options: BatchOptions, mut emit: F)
where
    F: FnMut(BatchResult),
{
    if paths.is_empty() {
        return;
    }
    let worker_count = options.jobs.max(1).min(paths.len());
    if worker_count == 1 {
        for path in paths.iter().cloned() {
            emit(BatchResult {
                result: read_with_limits(&path, options.limits),
                path,
            });
        }
        return;
    }

    let shared_paths = Arc::new(paths.to_vec());
    let next_index = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::sync_channel(worker_count);
    let mut pending = BTreeMap::new();
    let mut next_to_emit = 0_usize;

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let paths = Arc::clone(&shared_paths);
            let next_index = Arc::clone(&next_index);
            let sender = sender.clone();
            scope.spawn(move || {
                loop {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index).cloned() else {
                        break;
                    };
                    let result = read_with_limits(&path, options.limits);
                    if sender.send((index, BatchResult { path, result })).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (index, result) in receiver {
            pending.insert(index, result);
            while let Some(result) = pending.remove(&next_to_emit) {
                emit(result);
                next_to_emit += 1;
            }
        }
    });

    while let Some(result) = pending.remove(&next_to_emit) {
        emit(result);
        next_to_emit += 1;
    }
}

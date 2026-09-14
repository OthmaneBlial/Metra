//! Public library facade for Metra.
//!
//! The command-line application is deliberately a thin consumer of this API.
//! Format-specific parsing lives in `metra-formats`; stable data types and
//! safety limits live in `metra-core`.

use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

pub use metra_core::{
    CapabilityStatus, FileFormat, FileInfo, FormatCapabilities, Metadata, MetadataDiff, MetraError,
    ParseLimits, Result, Source, Tag, TagDefinition, TagDifference, TagValue, ValueType, Warning,
    format_capabilities, format_capabilities_all, tag_definition, tag_definitions,
};
pub use metra_formats::{
    AviCreateEntry, AviCreateOptions, AviEdit, CreateRequest, DetectedFormat, DngCreateOptions,
    FlacCreateEntry, FlacCreateOptions, FlacEdit, FormatHandler, GifCreateEntry, GifCreateOptions,
    GifEdit, IccCreateEntry, IccCreateOptions, IccEdit, IsobmffCreateEntry, IsobmffCreateKind,
    IsobmffCreateOptions, IsobmffEdit, JpegCreateOptions, JpegEdit, MatroskaCreateEntry,
    MatroskaCreateKind, MatroskaCreateOptions, MatroskaEdit, MetadataEdit, Mp3CreateEntry,
    Mp3CreateOptions, Mp3Edit, OggCreateEntry, OggCreateOptions, OggEdit, PdfCreateEntry,
    PdfCreateOptions, PdfEdit, PngCreateEntry, PngCreateOptions, PngEdit, PsdCreateOptions,
    PsdEdit, ReadSeek, SvgCreateOptions, SvgEdit, TiffCreateEntry, TiffCreateOptions, TiffEdit,
    WavCreateEntry, WavCreateOptions, WavEdit, WebpCreateOptions, WebpEdit, WriteSeek, XmpEdit,
    copy_metadata_path, create_avi_path, create_avi_to_vec, create_dng_path, create_dng_to_vec,
    create_flac_path, create_flac_to_vec, create_gif_path, create_gif_to_vec, create_icc_path,
    create_icc_to_vec, create_isobmff_path, create_isobmff_to_vec, create_jpeg_path,
    create_jpeg_to_vec, create_matroska_path, create_matroska_to_vec, create_mp3_path,
    create_mp3_to_vec, create_ogg_path, create_ogg_to_vec, create_path, create_pdf_path,
    create_pdf_to_vec, create_png_path, create_png_to_vec, create_psd_path, create_psd_to_vec,
    create_svg_path, create_svg_to_vec, create_tiff_path, create_tiff_to_vec, create_to_vec,
    create_wav_path, create_wav_to_vec, create_webp_path, create_webp_to_vec, create_xmp_path,
    create_xmp_to_vec, detect_format, format_handlers, handler_for_format, read_icc, read_ogg,
    read_path_with_limits, read_reader, read_reader_with_limits, read_xmp, rewrite_avi,
    rewrite_avi_path, rewrite_avi_to_vec, rewrite_flac, rewrite_flac_path, rewrite_flac_to_vec,
    rewrite_gif, rewrite_gif_path, rewrite_gif_to_vec, rewrite_icc, rewrite_icc_path,
    rewrite_icc_to_vec, rewrite_isobmff, rewrite_isobmff_path, rewrite_isobmff_to_vec,
    rewrite_jpeg, rewrite_jpeg_path, rewrite_jpeg_to_vec, rewrite_matroska, rewrite_matroska_path,
    rewrite_matroska_to_vec, rewrite_metadata_path, rewrite_metadata_to_vec, rewrite_mp3,
    rewrite_mp3_path, rewrite_mp3_to_vec, rewrite_ogg, rewrite_ogg_path, rewrite_ogg_to_vec,
    rewrite_pdf, rewrite_pdf_path, rewrite_pdf_to_vec, rewrite_png, rewrite_png_path,
    rewrite_png_to_vec, rewrite_psd, rewrite_psd_path, rewrite_psd_to_vec, rewrite_raw_cr3,
    rewrite_raw_cr3_path, rewrite_raw_cr3_to_vec, rewrite_raw_tiff, rewrite_raw_tiff_path,
    rewrite_raw_tiff_to_vec, rewrite_svg, rewrite_svg_path, rewrite_svg_to_vec, rewrite_tiff,
    rewrite_tiff_path, rewrite_tiff_to_vec, rewrite_wav, rewrite_wav_path, rewrite_wav_to_vec,
    rewrite_webp, rewrite_webp_path, rewrite_webp_to_vec, rewrite_xmp, rewrite_xmp_path,
    rewrite_xmp_to_vec,
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

/// Cooperative cancellation shared by long-running batch operations.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl CancellationToken {
    /// Create a token in the active state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation. In-flight parsing finishes its current bounded
    /// operation; no later batch item is started.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Return whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Inspect independent paths with bounded worker concurrency.
///
/// Results always follow the order of `paths`, including when workers finish
/// out of order. The returned vector intentionally contains one result per
/// input, so callers that need bounded output should use
/// [`read_many_streaming`] instead.
pub fn read_many(paths: &[PathBuf], options: BatchOptions) -> Vec<BatchResult> {
    read_many_internal(paths, options, None)
}

/// Inspect paths with cooperative cancellation support.
///
/// Paths already being parsed may finish, but no new file is read after the
/// token is cancelled. Remaining paths are returned as
/// [`MetraError::Cancelled`] results so callers retain one result per input.
pub fn read_many_with_cancellation(
    paths: &[PathBuf],
    options: BatchOptions,
    cancellation: &CancellationToken,
) -> Vec<BatchResult> {
    read_many_internal(paths, options, Some(cancellation))
}

fn read_many_internal(
    paths: &[PathBuf],
    options: BatchOptions,
    cancellation: Option<&CancellationToken>,
) -> Vec<BatchResult> {
    if paths.is_empty() {
        return Vec::new();
    }
    let worker_count = options.jobs.max(1).min(paths.len());
    if worker_count == 1 {
        return paths
            .iter()
            .cloned()
            .map(|path| BatchResult {
                result: read_one(&path, options.limits, cancellation),
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
                    let result = read_one(&path, options.limits, cancellation);
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

fn read_one(
    path: &Path,
    limits: ParseLimits,
    cancellation: Option<&CancellationToken>,
) -> Result<Metadata> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(MetraError::Cancelled)
    } else {
        read_with_limits(path, limits)
    }
}

/// Inspect paths in deterministic order while emitting each result as soon as
/// all earlier paths are ready.
///
/// Parallel workers use a bounded synchronous channel. If an early path is
/// slow, later results apply backpressure instead of growing an unbounded
/// out-of-order buffer.
pub fn read_many_streaming<F>(paths: &[PathBuf], options: BatchOptions, emit: F)
where
    F: FnMut(BatchResult),
{
    read_many_streaming_internal(paths, options, None, emit);
}

/// Stream deterministic batch results with cooperative cancellation support.
///
/// Once cancelled, currently active items may finish and remaining paths are
/// emitted as [`MetraError::Cancelled`] without being opened.
pub fn read_many_streaming_with_cancellation<F>(
    paths: &[PathBuf],
    options: BatchOptions,
    cancellation: &CancellationToken,
    emit: F,
) where
    F: FnMut(BatchResult),
{
    read_many_streaming_internal(paths, options, Some(cancellation), emit);
}

fn read_many_streaming_internal<F>(
    paths: &[PathBuf],
    options: BatchOptions,
    cancellation: Option<&CancellationToken>,
    mut emit: F,
) where
    F: FnMut(BatchResult),
{
    if paths.is_empty() {
        return;
    }
    let worker_count = options.jobs.max(1).min(paths.len());
    if worker_count == 1 {
        for path in paths.iter().cloned() {
            emit(BatchResult {
                result: read_one(&path, options.limits, cancellation),
                path,
            });
        }
        return;
    }

    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        for path in paths.iter().cloned() {
            emit(BatchResult {
                path,
                result: Err(MetraError::Cancelled),
            });
        }
        return;
    }

    let shared_paths = Arc::new(paths.to_vec());
    let (job_sender, job_receiver) = mpsc::sync_channel::<(usize, PathBuf)>(worker_count);
    let job_receiver = Arc::new(Mutex::new(job_receiver));
    let (result_sender, receiver) = mpsc::channel();
    let mut pending = BTreeMap::new();
    let mut next_to_emit = 0_usize;
    let mut next_to_schedule = worker_count;
    let mut job_sender = Some(job_sender);
    let cancellation = cancellation.cloned();

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let job_receiver = Arc::clone(&job_receiver);
            let sender = result_sender.clone();
            let worker_cancellation = cancellation.clone();
            scope.spawn(move || {
                loop {
                    let job = job_receiver
                        .lock()
                        .expect("batch job queue lock should not be poisoned")
                        .recv();
                    let Ok((index, path)) = job else {
                        break;
                    };
                    let result = read_one(&path, options.limits, worker_cancellation.as_ref());
                    if sender.send((index, BatchResult { path, result })).is_err() {
                        break;
                    }
                }
            });
        }

        drop(result_sender);
        for index in 0..worker_count {
            let path = shared_paths[index].clone();
            job_sender
                .as_ref()
                .expect("batch job sender should be available")
                .send((index, path))
                .expect("batch workers should accept initial jobs");
        }
        if next_to_schedule == shared_paths.len() {
            job_sender.take();
        }

        for (index, result) in receiver {
            pending.insert(index, result);
            while let Some(result) = pending.remove(&next_to_emit) {
                emit(result);
                next_to_emit += 1;
                if cancellation
                    .as_ref()
                    .is_some_and(CancellationToken::is_cancelled)
                {
                    job_sender.take();
                } else if next_to_schedule < shared_paths.len() {
                    let path = shared_paths[next_to_schedule].clone();
                    job_sender
                        .as_ref()
                        .expect("batch job sender should be available")
                        .send((next_to_schedule, path))
                        .expect("batch workers should accept scheduled jobs");
                    next_to_schedule += 1;
                    if next_to_schedule == shared_paths.len() {
                        job_sender.take();
                    }
                }
            }
        }
    });

    while let Some(result) = pending.remove(&next_to_emit) {
        emit(result);
        next_to_emit += 1;
    }

    if cancellation
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        while next_to_schedule < shared_paths.len() {
            emit(BatchResult {
                path: shared_paths[next_to_schedule].clone(),
                result: Err(MetraError::Cancelled),
            });
            next_to_schedule += 1;
        }
    }
}

//! Public library facade for Metra.
//!
//! The command-line application is deliberately a thin consumer of this API.
//! Format-specific parsing lives in `metra-formats`; stable data types and
//! safety limits live in `metra-core`.

use std::path::Path;

pub use metra_core::{
    FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue,
    ValueType, Warning,
};
pub use metra_formats::{
    DetectedFormat, FlacEdit, GifEdit, JpegEdit, Mp3Edit, PngEdit, WavEdit, detect_format,
    read_path_with_limits, rewrite_flac, rewrite_flac_path, rewrite_flac_to_vec, rewrite_gif,
    rewrite_gif_path, rewrite_gif_to_vec, rewrite_jpeg, rewrite_jpeg_path, rewrite_jpeg_to_vec,
    rewrite_mp3, rewrite_mp3_path, rewrite_mp3_to_vec, rewrite_png, rewrite_png_path,
    rewrite_png_to_vec, rewrite_wav, rewrite_wav_path, rewrite_wav_to_vec, rewrite_webp,
    rewrite_webp_path, rewrite_webp_to_vec,
};

/// Read metadata from a path using the default defensive parser limits.
pub fn read(path: impl AsRef<Path>) -> Result<Metadata> {
    metra_formats::read_path(path)
}

/// Read metadata from a path with caller-selected resource limits.
pub fn read_with_limits(path: impl AsRef<Path>, limits: ParseLimits) -> Result<Metadata> {
    read_path_with_limits(path, limits)
}

//! Public library facade for Metra.
//!
//! The command-line application is deliberately a thin consumer of this API.
//! Format-specific parsing lives in `metra-formats`; stable data types and
//! safety limits live in `metra-core`.

use std::io::{Read, Seek};
use std::path::Path;

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

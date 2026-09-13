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
pub use metra_formats::{DetectedFormat, detect_format, read_path_with_limits};

/// Read metadata from a path using the default defensive parser limits.
pub fn read(path: impl AsRef<Path>) -> Result<Metadata> {
    metra_formats::read_path(path)
}

/// Read metadata from a path with caller-selected resource limits.
pub fn read_with_limits(path: impl AsRef<Path>, limits: ParseLimits) -> Result<Metadata> {
    read_path_with_limits(path, limits)
}

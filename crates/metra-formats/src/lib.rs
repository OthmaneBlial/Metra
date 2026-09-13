//! Container detection and format readers.
//!
//! The public entry point intentionally accepts a path and returns the same
//! format-independent model for every reader. Format modules do not invoke
//! external tools and are safe to use with untrusted files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

mod gif;
mod icc;
mod iptc;
mod jpeg;
mod png;
mod tiff;
mod webp;
mod xmp;

pub use gif::read_gif;
pub use jpeg::read_jpeg;
pub use png::read_png;
pub use tiff::read_tiff;
pub use webp::read_webp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedFormat {
    pub format: FileFormat,
    pub signature: &'static str,
}

/// Detect a format from magic bytes. Extensions are deliberately not used as
/// the primary signal.
pub fn detect_format(bytes: &[u8]) -> Option<DetectedFormat> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(DetectedFormat {
            format: FileFormat::Jpeg,
            signature: "JPEG SOI",
        })
    } else if bytes.starts_with(b"\x89PNG\r\n\x1A\n") {
        Some(DetectedFormat {
            format: FileFormat::Png,
            signature: "PNG signature",
        })
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(DetectedFormat {
            format: FileFormat::Tiff,
            signature: "TIFF header",
        })
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(DetectedFormat {
            format: FileFormat::Webp,
            signature: "RIFF/WEBP",
        })
    } else if bytes.starts_with(b"%PDF-") {
        Some(DetectedFormat {
            format: FileFormat::Pdf,
            signature: "PDF header",
        })
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(DetectedFormat {
            format: FileFormat::Gif,
            signature: "GIF header",
        })
    } else {
        None
    }
}

pub fn read_path(path: impl AsRef<Path>) -> Result<Metadata> {
    read_path_with_limits(path, ParseLimits::default())
}

pub fn read_path_with_limits(path: impl AsRef<Path>, limits: ParseLimits) -> Result<Metadata> {
    let path = path.as_ref().to_path_buf();
    let mut file = File::open(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let size = file
        .metadata()
        .map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?
        .len();

    let mut header = [0_u8; 16];
    let header_len = file.read(&mut header).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    file.seek(SeekFrom::Start(0))
        .map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?;

    let detected =
        detect_format(&header[..header_len]).ok_or_else(|| MetraError::UnsupportedFormat {
            description: format!("unrecognized file signature for {}", path.display()),
        })?;
    let file_info = FileInfo::new(path.clone(), size, detected.format);

    match detected.format {
        FileFormat::Jpeg => jpeg::read_jpeg(&mut file, file_info, limits),
        FileFormat::Tiff => tiff::read_tiff(&mut file, file_info, limits),
        FileFormat::Png => png::read_png(&mut file, file_info, limits),
        FileFormat::Webp => webp::read_webp(&mut file, file_info, limits),
        FileFormat::Gif => gif::read_gif(&mut file, file_info, limits),
        format => Err(MetraError::UnsupportedFormat {
            description: format!("{format} is detected but its reader is not implemented yet"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_uses_signatures() {
        assert_eq!(
            detect_format(b"\xFF\xD8\xFFrest").unwrap().format,
            FileFormat::Jpeg
        );
        assert_eq!(
            detect_format(b"\x89PNG\r\n\x1A\nrest").unwrap().format,
            FileFormat::Png
        );
        assert_eq!(
            detect_format(b"II*\0rest").unwrap().format,
            FileFormat::Tiff
        );
        assert!(detect_format(b"photo.jpg").is_none());
    }
}

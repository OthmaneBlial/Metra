//! Container detection and format readers.
//!
//! The public entry point intentionally accepts a path and returns the same
//! format-independent model for every reader. Format modules do not invoke
//! external tools and are safe to use with untrusted files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

mod avi;
mod flac;
mod flac_writer;
mod gif;
mod gif_writer;
mod icc;
mod id3;
mod id3_writer;
mod inflate;
mod iptc;
mod iptc_writer;
mod isobmff;
mod jpeg;
mod makers;
mod matroska;
mod pdf;
mod png;
mod png_writer;
mod psd;
mod svg;
mod svg_writer;
mod tiff;
mod wav;
mod wav_writer;
mod webp;
mod webp_writer;
mod xml;
mod xmp;

pub use avi::read_avi;
pub use flac::read_flac;
pub use flac_writer::{FlacEdit, rewrite_flac, rewrite_flac_path, rewrite_flac_to_vec};
pub use gif::read_gif;
pub use gif_writer::{GifEdit, rewrite_gif, rewrite_gif_path, rewrite_gif_to_vec};
pub use id3::read_mp3;
pub use id3_writer::{Mp3Edit, rewrite_mp3, rewrite_mp3_path, rewrite_mp3_to_vec};
pub use isobmff::read_isobmff;
pub use jpeg::{JpegEdit, read_jpeg, rewrite_jpeg, rewrite_jpeg_path, rewrite_jpeg_to_vec};
pub use matroska::read_matroska;
pub use pdf::read_pdf;
pub use png::read_png;
pub use png_writer::{PngEdit, rewrite_png, rewrite_png_path, rewrite_png_to_vec};
pub use psd::read_psd;
pub use svg::read_svg;
pub use svg_writer::{SvgEdit, rewrite_svg, rewrite_svg_path, rewrite_svg_to_vec};
pub use tiff::read_tiff;
pub use wav::read_wav;
pub use wav_writer::{WavEdit, rewrite_wav, rewrite_wav_path, rewrite_wav_to_vec};
pub use webp::read_webp;
pub use webp_writer::{WebpEdit, rewrite_webp, rewrite_webp_path, rewrite_webp_to_vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedFormat {
    pub format: FileFormat,
    pub signature: &'static str,
}

/// Detect a format from magic bytes. Extensions are deliberately not used as
/// the primary signal.
pub fn detect_format(bytes: &[u8]) -> Option<DetectedFormat> {
    if bytes.starts_with(b"8BPS") {
        Some(DetectedFormat {
            format: FileFormat::Psd,
            signature: "PSD/PSB header",
        })
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
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
    } else if bytes.starts_with(b"\x1A\x45\xDF\xA3") {
        let format = if matroska::document_type(bytes).as_deref() == Some("webm") {
            FileFormat::Webm
        } else {
            FileFormat::Mkv
        };
        Some(DetectedFormat {
            format,
            signature: "EBML/Matroska header",
        })
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" {
        if &bytes[8..12] == b"WEBP" {
            Some(DetectedFormat {
                format: FileFormat::Webp,
                signature: "RIFF/WEBP",
            })
        } else if &bytes[8..12] == b"WAVE" {
            Some(DetectedFormat {
                format: FileFormat::Wav,
                signature: "RIFF/WAVE",
            })
        } else if &bytes[8..12] == b"AVI " {
            Some(DetectedFormat {
                format: FileFormat::Avi,
                signature: "RIFF/AVI",
            })
        } else {
            None
        }
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let brand = &bytes[8..12];
        let (format, signature) = match brand {
            b"avif" | b"avis" => (FileFormat::Avif, "ISO-BMFF ftyp/AVIF"),
            b"heic" | b"heix" | b"hevc" | b"heim" | b"heis" | b"mif1" | b"msf1" => {
                (FileFormat::Heif, "ISO-BMFF ftyp/HEIF")
            }
            b"qt  " => (FileFormat::Mov, "ISO-BMFF ftyp/QuickTime"),
            b"M4A " | b"M4B " => (FileFormat::M4a, "ISO-BMFF ftyp/M4A"),
            _ => (FileFormat::Mp4, "ISO-BMFF ftyp/MP4"),
        };
        Some(DetectedFormat { format, signature })
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
    } else if bytes.starts_with(b"ID3") {
        Some(DetectedFormat {
            format: FileFormat::Mp3,
            signature: "ID3 header",
        })
    } else if bytes.starts_with(b"fLaC") {
        Some(DetectedFormat {
            format: FileFormat::Flac,
            signature: "FLAC signature",
        })
    } else if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0 {
        Some(DetectedFormat {
            format: FileFormat::Mp3,
            signature: "MPEG audio frame sync",
        })
    } else if is_svg_signature(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Svg,
            signature: "SVG XML root",
        })
    } else {
        None
    }
}

fn is_svg_signature(bytes: &[u8]) -> bool {
    let mut cursor = 0_usize;
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        cursor = 3;
    }
    loop {
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if bytes
            .get(cursor..)
            .is_some_and(|rest| rest.starts_with(b"<!--"))
        {
            let Some(end) = bytes[cursor + 4..]
                .windows(3)
                .position(|window| window == b"-->")
            else {
                return false;
            };
            cursor += 4 + end + 3;
            continue;
        }
        if bytes
            .get(cursor..)
            .is_some_and(|rest| rest.starts_with(b"<?xml"))
        {
            let Some(end) = bytes[cursor + 5..]
                .windows(2)
                .position(|window| window == b"?>")
            else {
                return false;
            };
            cursor += 5 + end + 2;
            continue;
        }
        break;
    }
    let Some(rest) = bytes.get(cursor..) else {
        return false;
    };
    if !rest.starts_with(b"<svg") {
        return false;
    }
    rest.get(4)
        .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'))
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

    let mut header = [0_u8; 4096];
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
        FileFormat::Heif
        | FileFormat::Avif
        | FileFormat::Mp4
        | FileFormat::Mov
        | FileFormat::M4a => isobmff::read_isobmff(&mut file, file_info, limits),
        FileFormat::Mp3 => id3::read_mp3(&mut file, file_info, limits),
        FileFormat::Flac => flac::read_flac(&mut file, file_info, limits),
        FileFormat::Pdf => pdf::read_pdf(&mut file, file_info, limits),
        FileFormat::Wav => wav::read_wav(&mut file, file_info, limits),
        FileFormat::Svg => svg::read_svg(&mut file, file_info, limits),
        FileFormat::Psd => psd::read_psd(&mut file, file_info, limits),
        FileFormat::Avi => avi::read_avi(&mut file, file_info, limits),
        FileFormat::Mkv | FileFormat::Webm => matroska::read_matroska(&mut file, file_info, limits),
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
        assert_eq!(
            detect_format(b"8BPS\0\x01rest").unwrap().format,
            FileFormat::Psd
        );
        assert_eq!(
            detect_format(b"RIFF\0\0\0\0AVI ").unwrap().format,
            FileFormat::Avi
        );
        assert_eq!(
            detect_format(b"\x1A\x45\xDF\xA3\x9F\x42\x82\x84webm")
                .unwrap()
                .format,
            FileFormat::Webm
        );
        assert!(detect_format(b"photo.jpg").is_none());
        assert_eq!(
            detect_format(b"ID3\x04\0\0\0\0\0\0\0").unwrap().format,
            FileFormat::Mp3
        );
        assert_eq!(
            detect_format(b"\xFF\xFB\x90\x64").unwrap().format,
            FileFormat::Mp3
        );
        assert_eq!(
            detect_format(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\">")
                .unwrap()
                .format,
            FileFormat::Svg
        );
    }
}

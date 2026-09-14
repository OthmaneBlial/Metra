//! Container detection and format readers.
//!
//! The public entry point intentionally accepts a path and returns the same
//! format-independent model for every reader. Format modules do not invoke
//! external tools and are safe to use with untrusted files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{FileFormat, FileInfo, Metadata, MetraError, ParseLimits, Result};

mod atomic;
mod avi;
mod avi_writer;
mod edit;
mod flac;
mod flac_create;
mod flac_writer;
mod gif;
mod gif_create;
mod gif_writer;
mod handler;
mod icc;
mod icc_create;
mod id3;
mod id3_writer;
mod inflate;
mod iptc;
mod iptc_writer;
mod isobmff;
mod isobmff_writer;
mod jpeg;
mod makers;
mod matroska;
mod matroska_writer;
mod ogg;
mod ogg_writer;
mod pdf;
mod pdf_writer;
mod png;
mod png_create;
mod png_writer;
mod psd;
mod psd_writer;
mod raw;
mod raw_cr3_writer;
mod raw_writer;
mod svg;
mod svg_writer;
mod tiff;
mod tiff_create;
mod tiff_writer;
mod wav;
mod wav_create;
mod wav_writer;
mod webp;
mod webp_writer;
mod xml;
mod xmp;
mod xmp_create;

pub use avi::read_avi;
pub use avi_writer::{AviEdit, rewrite_avi, rewrite_avi_path, rewrite_avi_to_vec};
pub use edit::{MetadataEdit, copy_metadata_path, rewrite_metadata_path, rewrite_metadata_to_vec};
pub use flac::read_flac;
pub use flac_create::{FlacCreateEntry, FlacCreateOptions, create_flac_path, create_flac_to_vec};
pub use flac_writer::{FlacEdit, rewrite_flac, rewrite_flac_path, rewrite_flac_to_vec};
pub use gif::read_gif;
pub use gif_create::{GifCreateEntry, GifCreateOptions, create_gif_path, create_gif_to_vec};
pub use gif_writer::{GifEdit, rewrite_gif, rewrite_gif_path, rewrite_gif_to_vec};
pub use handler::{FormatHandler, ReadSeek, WriteSeek, format_handlers, handler_for_format};
pub use icc::read_icc;
pub use icc_create::{IccCreateEntry, IccCreateOptions, create_icc_path, create_icc_to_vec};
pub use id3::read_mp3;
pub use id3_writer::{Mp3Edit, rewrite_mp3, rewrite_mp3_path, rewrite_mp3_to_vec};
pub use isobmff::read_isobmff;
pub use isobmff_writer::{
    IsobmffEdit, rewrite_isobmff, rewrite_isobmff_path, rewrite_isobmff_to_vec,
};
pub use jpeg::{JpegEdit, read_jpeg, rewrite_jpeg, rewrite_jpeg_path, rewrite_jpeg_to_vec};
pub use matroska::read_matroska;
pub use matroska_writer::{
    MatroskaEdit, rewrite_matroska, rewrite_matroska_path, rewrite_matroska_to_vec,
};
pub use ogg::read_ogg;
pub use ogg_writer::{OggEdit, rewrite_ogg, rewrite_ogg_path, rewrite_ogg_to_vec};
pub use pdf::read_pdf;
pub use pdf_writer::{PdfEdit, rewrite_pdf, rewrite_pdf_path, rewrite_pdf_to_vec};
pub use png::read_png;
pub use png_create::{PngCreateEntry, PngCreateOptions, create_png_path, create_png_to_vec};
pub use png_writer::{PngEdit, rewrite_png, rewrite_png_path, rewrite_png_to_vec};
pub use psd::read_psd;
pub use psd_writer::{PsdEdit, rewrite_psd, rewrite_psd_path, rewrite_psd_to_vec};
pub use raw::read_raw;
pub use raw_cr3_writer::{rewrite_raw_cr3, rewrite_raw_cr3_path, rewrite_raw_cr3_to_vec};
pub use raw_writer::{rewrite_raw_tiff, rewrite_raw_tiff_path, rewrite_raw_tiff_to_vec};
pub use svg::read_svg;
pub use svg_writer::{SvgEdit, rewrite_svg, rewrite_svg_path, rewrite_svg_to_vec};
pub use tiff::read_tiff;
pub use tiff_create::{TiffCreateEntry, TiffCreateOptions, create_tiff_path, create_tiff_to_vec};
pub use tiff_writer::{TiffEdit, rewrite_tiff, rewrite_tiff_path, rewrite_tiff_to_vec};
pub use wav::read_wav;
pub use wav_create::{WavCreateEntry, WavCreateOptions, create_wav_path, create_wav_to_vec};
pub use wav_writer::{WavEdit, rewrite_wav, rewrite_wav_path, rewrite_wav_to_vec};
pub use webp::read_webp;
pub use webp_writer::{WebpEdit, rewrite_webp, rewrite_webp_path, rewrite_webp_to_vec};
pub use xmp::read_xmp;
pub use xmp_create::{create_xmp_path, create_xmp_to_vec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedFormat {
    pub format: FileFormat,
    pub signature: &'static str,
}

/// Detect a format from magic bytes. Extensions are deliberately not used as
/// the primary signal.
pub fn detect_format(bytes: &[u8]) -> Option<DetectedFormat> {
    if raw::is_raf_header(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "RAF header",
        })
    } else if raw::is_crw_header(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "Canon CIFF/CRW header",
        })
    } else if raw::is_mrw_header(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "Minolta MRW header",
        })
    } else if raw::is_x3f_header(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "Sigma X3F header",
        })
    } else if raw::is_cr2_header(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "TIFF/CR2 header",
        })
    } else if raw::is_cr3_header(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "ISO-BMFF ftyp/CR3",
        })
    } else if bytes.starts_with(b"8BPS") {
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
    } else if bytes.starts_with(b"IIU\0") || bytes.starts_with(b"MM\0U") {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "TIFF/RW2 header",
        })
    } else if raw::is_tiff_header(bytes) {
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
    } else if ogg::is_ogg_signature(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Ogg,
            signature: "OggS container",
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
    } else if icc::is_icc_signature(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Icc,
            signature: "ICC profile",
        })
    } else if xmp::is_xmp_signature(bytes) {
        Some(DetectedFormat {
            format: FileFormat::Xmp,
            signature: "XMP packet",
        })
    } else {
        None
    }
}

fn detect_format_for_path(path: &Path, bytes: &[u8]) -> Option<DetectedFormat> {
    let detected = detect_format(bytes)?;
    if detected.format == FileFormat::Tiff && raw::raw_variant(path).is_some() {
        Some(DetectedFormat {
            format: FileFormat::Raw,
            signature: "TIFF/RAW extension",
        })
    } else {
        Some(detected)
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

pub(crate) fn read_bounded_document<R: Read + Seek>(
    reader: &mut R,
    file_info: &FileInfo,
    limits: ParseLimits,
    context: &str,
) -> Result<Vec<u8>> {
    let length =
        usize::try_from(file_info.size).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: context.to_owned(),
            limit: limits.max_value_bytes,
        })?;
    if length > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: context.to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| MetraError::Io {
            path: file_info.path.clone(),
            source,
        })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                context: context.to_owned(),
            },
            _ => MetraError::Io {
                path: file_info.path.clone(),
                source,
            },
        })?;
    Ok(bytes)
}

/// Read metadata from a seekable stream using the default defensive limits.
pub fn read_reader<R: Read + Seek>(reader: &mut R, file_info: FileInfo) -> Result<Metadata> {
    read_reader_with_limits(reader, file_info, ParseLimits::default())
}

/// Detect and read metadata from a seekable stream with caller-selected limits.
///
/// The caller supplies the stream path and declared size for diagnostics and
/// range checks. The format is detected from the stream signature rather than
/// trusted from `file_info.format`.
pub fn read_reader_with_limits<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let mut header = [0_u8; 4096];
    let mut header_len = 0;
    while header_len < header.len() {
        match reader
            .read(&mut header[header_len..])
            .map_err(|source| MetraError::Io {
                path: file_info.path.clone(),
                source,
            })? {
            0 => break,
            read => header_len += read,
        }
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|source| MetraError::Io {
            path: file_info.path.clone(),
            source,
        })?;

    let detected =
        detect_format_for_path(&file_info.path, &header[..header_len]).ok_or_else(|| {
            MetraError::UnsupportedFormat {
                description: format!(
                    "unrecognized file signature for {}",
                    file_info.path.display()
                ),
            }
        })?;
    let file_info = FileInfo::new(file_info.path.clone(), file_info.size, detected.format);

    handler::handler_for_format(detected.format)
        .ok_or_else(|| handler::unsupported_handler(detected.format))?
        .read_metadata(reader, file_info, limits)
}

pub fn read_path_with_limits(path: impl AsRef<Path>, limits: ParseLimits) -> Result<Metadata> {
    let path = path.as_ref().to_path_buf();
    let file = File::open(&path).map_err(|source| MetraError::Io {
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
    let mut file = file;
    read_reader_with_limits(
        &mut file,
        FileInfo::new(path, size, FileFormat::Unknown),
        limits,
    )
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
            detect_format(b"II+\0rest").unwrap().format,
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
            detect_format(b"II*\0\0\0\0\0CR\x02\0").unwrap().format,
            FileFormat::Raw
        );
        assert_eq!(
            detect_format(b"FUJIFILMCCD-RAW ").unwrap().format,
            FileFormat::Raw
        );
        assert_eq!(
            detect_format(b"II\x1A\0\0\0HEAPCCDR\0\0\0\0")
                .unwrap()
                .format,
            FileFormat::Raw
        );
        assert_eq!(
            detect_format(b"\0MRM\0\0\0\0").unwrap().format,
            FileFormat::Raw
        );
        assert_eq!(
            detect_format(b"FOVb\0\0\0\0").unwrap().format,
            FileFormat::Raw
        );
        assert_eq!(
            detect_format(b"\0\0\0\0ftypcrx \0\0\0\0").unwrap().format,
            FileFormat::Raw
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
            detect_format(b"OggS\0\0\0\0").unwrap().format,
            FileFormat::Ogg
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
        let mut icc = vec![0_u8; 132];
        icc[36..40].copy_from_slice(b"acsp");
        assert_eq!(detect_format(&icc).unwrap().format, FileFormat::Icc);
        assert_eq!(
            detect_format(b"<?xpacket begin=\"\"?><x:xmpmeta/>")
                .unwrap()
                .format,
            FileFormat::Xmp
        );
        assert_eq!(
            detect_format(b"<?xpacket?><xmp:xmpmeta/>").unwrap().format,
            FileFormat::Xmp
        );
        assert!(detect_format(b"\0binary payload <rdf:RDF>").is_none());
    }
}

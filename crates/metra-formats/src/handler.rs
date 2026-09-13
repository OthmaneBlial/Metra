use std::io::{Read, Seek};

use metra_core::{
    FileFormat, FileInfo, FormatCapabilities, Metadata, MetraError, ParseLimits, Result,
    format_capabilities,
};

use super::{DetectedFormat, detect_format};

/// A seekable reader object accepted by every registered format handler.
pub trait ReadSeek: Read + Seek {}

impl<T: Read + Seek + ?Sized> ReadSeek for T {}

/// Common read-side contract for an explicit Metra format handler.
pub trait FormatHandler: Sync {
    /// The format family handled by this entry.
    fn format(&self) -> FileFormat;

    /// Detect this format from a bounded signature prefix.
    fn detect(&self, bytes: &[u8]) -> Option<DetectedFormat>;

    /// Read metadata using the shared typed model and defensive limits.
    fn read_metadata(
        &self,
        reader: &mut dyn ReadSeek,
        file_info: FileInfo,
        limits: ParseLimits,
    ) -> Result<Metadata>;

    /// Report the current read/write/create/delete/rewrite/streaming surface.
    fn capabilities(&self) -> FormatCapabilities {
        format_capabilities(self.format())
    }
}

pub struct RegisteredFormatHandler {
    format: FileFormat,
    reader: fn(&mut dyn ReadSeek, FileInfo, ParseLimits) -> Result<Metadata>,
}

impl FormatHandler for RegisteredFormatHandler {
    fn format(&self) -> FileFormat {
        self.format
    }

    fn detect(&self, bytes: &[u8]) -> Option<DetectedFormat> {
        detect_format(bytes).filter(|detected| detected.format == self.format)
    }

    fn read_metadata(
        &self,
        reader: &mut dyn ReadSeek,
        file_info: FileInfo,
        limits: ParseLimits,
    ) -> Result<Metadata> {
        (self.reader)(reader, file_info, limits)
    }
}

macro_rules! reader_adapter {
    ($name:ident, $reader:path) => {
        fn $name(
            mut reader: &mut dyn ReadSeek,
            file_info: FileInfo,
            limits: ParseLimits,
        ) -> Result<Metadata> {
            $reader(&mut reader, file_info, limits)
        }
    };
}

reader_adapter!(read_avi, super::avi::read_avi);
reader_adapter!(read_flac, super::flac::read_flac);
reader_adapter!(read_gif, super::gif::read_gif);
reader_adapter!(read_icc, super::icc::read_icc);
reader_adapter!(read_jpeg, super::jpeg::read_jpeg);
reader_adapter!(read_matroska, super::matroska::read_matroska);
reader_adapter!(read_mp3, super::id3::read_mp3);
reader_adapter!(read_ogg, super::ogg::read_ogg);
reader_adapter!(read_pdf, super::pdf::read_pdf);
reader_adapter!(read_png, super::png::read_png);
reader_adapter!(read_psd, super::psd::read_psd);
reader_adapter!(read_raw, super::raw::read_raw);
reader_adapter!(read_svg, super::svg::read_svg);
reader_adapter!(read_tiff, super::tiff::read_tiff);
reader_adapter!(read_wav, super::wav::read_wav);
reader_adapter!(read_webp, super::webp::read_webp);
reader_adapter!(read_xmp, super::xmp::read_xmp);
reader_adapter!(read_isobmff, super::isobmff::read_isobmff);

static FORMAT_HANDLERS: &[RegisteredFormatHandler] = &[
    RegisteredFormatHandler {
        format: FileFormat::Jpeg,
        reader: read_jpeg,
    },
    RegisteredFormatHandler {
        format: FileFormat::Tiff,
        reader: read_tiff,
    },
    RegisteredFormatHandler {
        format: FileFormat::Png,
        reader: read_png,
    },
    RegisteredFormatHandler {
        format: FileFormat::Webp,
        reader: read_webp,
    },
    RegisteredFormatHandler {
        format: FileFormat::Heif,
        reader: read_isobmff,
    },
    RegisteredFormatHandler {
        format: FileFormat::Avif,
        reader: read_isobmff,
    },
    RegisteredFormatHandler {
        format: FileFormat::Mp4,
        reader: read_isobmff,
    },
    RegisteredFormatHandler {
        format: FileFormat::Mov,
        reader: read_isobmff,
    },
    RegisteredFormatHandler {
        format: FileFormat::M4a,
        reader: read_isobmff,
    },
    RegisteredFormatHandler {
        format: FileFormat::Pdf,
        reader: read_pdf,
    },
    RegisteredFormatHandler {
        format: FileFormat::Gif,
        reader: read_gif,
    },
    RegisteredFormatHandler {
        format: FileFormat::Mp3,
        reader: read_mp3,
    },
    RegisteredFormatHandler {
        format: FileFormat::Flac,
        reader: read_flac,
    },
    RegisteredFormatHandler {
        format: FileFormat::Ogg,
        reader: read_ogg,
    },
    RegisteredFormatHandler {
        format: FileFormat::Wav,
        reader: read_wav,
    },
    RegisteredFormatHandler {
        format: FileFormat::Svg,
        reader: read_svg,
    },
    RegisteredFormatHandler {
        format: FileFormat::Icc,
        reader: read_icc,
    },
    RegisteredFormatHandler {
        format: FileFormat::Xmp,
        reader: read_xmp,
    },
    RegisteredFormatHandler {
        format: FileFormat::Psd,
        reader: read_psd,
    },
    RegisteredFormatHandler {
        format: FileFormat::Avi,
        reader: read_avi,
    },
    RegisteredFormatHandler {
        format: FileFormat::Mkv,
        reader: read_matroska,
    },
    RegisteredFormatHandler {
        format: FileFormat::Webm,
        reader: read_matroska,
    },
    RegisteredFormatHandler {
        format: FileFormat::Raw,
        reader: read_raw,
    },
];

pub fn format_handlers() -> &'static [&'static dyn FormatHandler] {
    // Keep the registry as trait objects at the API boundary while retaining
    // a single static allocation for the concrete entries.
    static HANDLER_REFS: std::sync::OnceLock<Vec<&'static dyn FormatHandler>> =
        std::sync::OnceLock::new();
    HANDLER_REFS.get_or_init(|| {
        FORMAT_HANDLERS
            .iter()
            .map(|handler| handler as &dyn FormatHandler)
            .collect()
    })
}

pub fn handler_for_format(format: FileFormat) -> Option<&'static dyn FormatHandler> {
    format_handlers()
        .iter()
        .copied()
        .find(|handler| handler.format() == format)
}

pub fn unsupported_handler(format: FileFormat) -> MetraError {
    MetraError::UnsupportedFormat {
        description: format!("{format} is detected but its reader is not implemented yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_every_core_format_with_a_reader() {
        assert_eq!(format_handlers().len(), 23);
        for capabilities in metra_core::format_capabilities_all() {
            assert_eq!(
                handler_for_format(capabilities.format)
                    .expect("every capability entry should have a reader")
                    .capabilities(),
                *capabilities
            );
        }
    }

    #[test]
    fn handlers_detect_only_their_registered_format() {
        let jpeg = handler_for_format(FileFormat::Jpeg).expect("JPEG handler should exist");
        assert_eq!(
            jpeg.detect(b"\xFF\xD8\xFFpayload").unwrap().format,
            FileFormat::Jpeg
        );
        assert!(jpeg.detect(b"\x89PNG\r\n\x1A\npayload").is_none());
    }
}

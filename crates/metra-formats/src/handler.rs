use std::io::{Read, Seek, Write};

use metra_core::{
    FileFormat, FileInfo, FormatCapabilities, Metadata, MetraError, ParseLimits, Result,
    format_capabilities,
};

use super::{DetectedFormat, MetadataEdit, detect_format};

/// A seekable reader object accepted by every registered format handler.
pub trait ReadSeek: Read + Seek {}

impl<T: Read + Seek + ?Sized> ReadSeek for T {}

/// A seekable output object accepted by format writers.
pub trait WriteSeek: Write + Seek {}

impl<T: Write + Seek + ?Sized> WriteSeek for T {}

type WriterAdapter =
    fn(&mut dyn ReadSeek, &mut dyn WriteSeek, FileInfo, ParseLimits, &[MetadataEdit]) -> Result<()>;

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

    /// Rewrite the supplied canonical edits into a seekable output stream.
    ///
    /// Handlers without a validated writer return an explicit unsupported
    /// error. The operation does not imply metadata creation or arbitrary
    /// typed-tag mutation; those capabilities remain reported separately.
    fn write_metadata(
        &self,
        reader: &mut dyn ReadSeek,
        writer: &mut dyn WriteSeek,
        file_info: FileInfo,
        limits: ParseLimits,
        edits: &[MetadataEdit],
    ) -> Result<()> {
        let _ = (reader, writer, file_info, limits, edits);
        Err(unsupported_writer(self.format()))
    }

    /// Report the current read/write/create/delete/rewrite/streaming surface.
    fn capabilities(&self) -> FormatCapabilities {
        format_capabilities(self.format())
    }
}

pub struct RegisteredFormatHandler {
    format: FileFormat,
    reader: fn(&mut dyn ReadSeek, FileInfo, ParseLimits) -> Result<Metadata>,
    writer: Option<WriterAdapter>,
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

    fn write_metadata(
        &self,
        reader: &mut dyn ReadSeek,
        writer: &mut dyn WriteSeek,
        file_info: FileInfo,
        limits: ParseLimits,
        edits: &[MetadataEdit],
    ) -> Result<()> {
        self.writer.ok_or_else(|| unsupported_writer(self.format))?(
            reader, writer, file_info, limits, edits,
        )
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

macro_rules! writer_adapter {
    ($name:ident, $writer:path, $collector:path) => {
        fn $name(
            mut reader: &mut dyn ReadSeek,
            mut writer: &mut dyn WriteSeek,
            file_info: FileInfo,
            limits: ParseLimits,
            edits: &[MetadataEdit],
        ) -> Result<()> {
            let edits = $collector(edits, file_info.format)?;
            $writer(&mut reader, &mut writer, file_info, limits, &edits)
        }
    };
}

writer_adapter!(
    write_jpeg,
    super::jpeg::rewrite_jpeg,
    super::edit::collect_jpeg
);
writer_adapter!(
    write_tiff,
    super::tiff_writer::rewrite_tiff,
    super::edit::collect_tiff
);
writer_adapter!(
    write_png,
    super::png_writer::rewrite_png,
    super::edit::collect_png
);
writer_adapter!(
    write_webp,
    super::webp_writer::rewrite_webp,
    super::edit::collect_webp
);
writer_adapter!(
    write_isobmff,
    super::isobmff_writer::rewrite_isobmff,
    super::edit::collect_isobmff
);
writer_adapter!(
    write_gif,
    super::gif_writer::rewrite_gif,
    super::edit::collect_gif
);
writer_adapter!(
    write_mp3,
    super::id3_writer::rewrite_mp3,
    super::edit::collect_mp3
);
writer_adapter!(
    write_flac,
    super::flac_writer::rewrite_flac,
    super::edit::collect_flac
);
writer_adapter!(
    write_ogg,
    super::ogg_writer::rewrite_ogg,
    super::edit::collect_ogg
);
writer_adapter!(
    write_pdf,
    super::pdf_writer::rewrite_pdf,
    super::edit::collect_pdf
);
writer_adapter!(
    write_psd,
    super::psd_writer::rewrite_psd,
    super::edit::collect_psd
);
writer_adapter!(
    write_avi,
    super::avi_writer::rewrite_avi,
    super::edit::collect_avi
);
writer_adapter!(
    write_wav,
    super::wav_writer::rewrite_wav,
    super::edit::collect_wav
);
writer_adapter!(
    write_svg,
    super::svg_writer::rewrite_svg,
    super::edit::collect_svg
);

static FORMAT_HANDLERS: &[RegisteredFormatHandler] = &[
    RegisteredFormatHandler {
        format: FileFormat::Jpeg,
        reader: read_jpeg,
        writer: Some(write_jpeg),
    },
    RegisteredFormatHandler {
        format: FileFormat::Tiff,
        reader: read_tiff,
        writer: Some(write_tiff),
    },
    RegisteredFormatHandler {
        format: FileFormat::Png,
        reader: read_png,
        writer: Some(write_png),
    },
    RegisteredFormatHandler {
        format: FileFormat::Webp,
        reader: read_webp,
        writer: Some(write_webp),
    },
    RegisteredFormatHandler {
        format: FileFormat::Heif,
        reader: read_isobmff,
        writer: Some(write_isobmff),
    },
    RegisteredFormatHandler {
        format: FileFormat::Avif,
        reader: read_isobmff,
        writer: Some(write_isobmff),
    },
    RegisteredFormatHandler {
        format: FileFormat::Mp4,
        reader: read_isobmff,
        writer: Some(write_isobmff),
    },
    RegisteredFormatHandler {
        format: FileFormat::Mov,
        reader: read_isobmff,
        writer: Some(write_isobmff),
    },
    RegisteredFormatHandler {
        format: FileFormat::M4a,
        reader: read_isobmff,
        writer: Some(write_isobmff),
    },
    RegisteredFormatHandler {
        format: FileFormat::Pdf,
        reader: read_pdf,
        writer: Some(write_pdf),
    },
    RegisteredFormatHandler {
        format: FileFormat::Psd,
        reader: read_psd,
        writer: Some(write_psd),
    },
    RegisteredFormatHandler {
        format: FileFormat::Avi,
        reader: read_avi,
        writer: Some(write_avi),
    },
    RegisteredFormatHandler {
        format: FileFormat::Gif,
        reader: read_gif,
        writer: Some(write_gif),
    },
    RegisteredFormatHandler {
        format: FileFormat::Mp3,
        reader: read_mp3,
        writer: Some(write_mp3),
    },
    RegisteredFormatHandler {
        format: FileFormat::Flac,
        reader: read_flac,
        writer: Some(write_flac),
    },
    RegisteredFormatHandler {
        format: FileFormat::Ogg,
        reader: read_ogg,
        writer: Some(write_ogg),
    },
    RegisteredFormatHandler {
        format: FileFormat::Wav,
        reader: read_wav,
        writer: Some(write_wav),
    },
    RegisteredFormatHandler {
        format: FileFormat::Svg,
        reader: read_svg,
        writer: Some(write_svg),
    },
    RegisteredFormatHandler {
        format: FileFormat::Icc,
        reader: read_icc,
        writer: None,
    },
    RegisteredFormatHandler {
        format: FileFormat::Xmp,
        reader: read_xmp,
        writer: None,
    },
    RegisteredFormatHandler {
        format: FileFormat::Mkv,
        reader: read_matroska,
        writer: None,
    },
    RegisteredFormatHandler {
        format: FileFormat::Webm,
        reader: read_matroska,
        writer: None,
    },
    RegisteredFormatHandler {
        format: FileFormat::Raw,
        reader: read_raw,
        writer: None,
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

fn unsupported_writer(format: FileFormat) -> MetraError {
    MetraError::UnsupportedFormat {
        description: format!("validated metadata writing is not implemented for {format}"),
    }
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

    #[test]
    fn registered_writer_dispatches_canonical_edits() {
        let bytes = [
            0xFF, 0xD8, // SOI
            0xFF, 0xFE, 0x00, 0x05, b'o', b'l', b'd', // COM
            0xFF, 0xD9, // EOI
        ];
        let handler = handler_for_format(FileFormat::Jpeg).expect("JPEG handler should exist");
        let mut reader = std::io::Cursor::new(bytes.to_vec());
        let mut writer = std::io::Cursor::new(Vec::new());
        handler
            .write_metadata(
                &mut reader,
                &mut writer,
                FileInfo::new("handler.jpg".into(), bytes.len() as u64, FileFormat::Jpeg),
                ParseLimits::default(),
                &[MetadataEdit::set("JPEG:Comment", "new")],
            )
            .expect("registered JPEG writer should accept canonical edits");

        let metadata = crate::read_reader(
            &mut std::io::Cursor::new(writer.into_inner()),
            FileInfo::new(
                "handler.jpg".into(),
                bytes.len() as u64,
                FileFormat::Unknown,
            ),
        )
        .expect("registered writer output should remain readable");
        assert_eq!(
            metadata.find("JPEG:Comment").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn registered_pdf_writer_dispatches_canonical_edits() {
        let bytes = b"%PDF-1.7\n5 0 obj\n<< /Title (Before) >>\nendobj\ntrailer\n<< /Info 5 0 R >>\nstartxref\n9\n%%EOF\n";
        let handler = handler_for_format(FileFormat::Pdf).expect("PDF handler should exist");
        let mut reader = std::io::Cursor::new(bytes.to_vec());
        let mut writer = std::io::Cursor::new(Vec::new());
        handler
            .write_metadata(
                &mut reader,
                &mut writer,
                FileInfo::new("handler.pdf".into(), bytes.len() as u64, FileFormat::Pdf),
                ParseLimits::default(),
                &[MetadataEdit::set("PDF:Title", "After!")],
            )
            .expect("registered PDF writer should accept canonical edits");

        let output = writer.into_inner();
        let metadata = crate::read_reader(
            &mut std::io::Cursor::new(output.clone()),
            FileInfo::new(
                "handler.pdf".into(),
                output.len() as u64,
                FileFormat::Unknown,
            ),
        )
        .expect("registered PDF writer output should remain readable");
        assert_eq!(
            metadata.find("PDF:Title").unwrap().display_value(),
            "After!"
        );
    }

    #[test]
    fn registered_psd_writer_dispatches_canonical_edits() {
        let packet = |format: &str| {
            format!(
                "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description xmlns:dc=\"urn:dc\" dc:format=\"{format}\"/></rdf:RDF></x:xmpmeta>"
            )
            .into_bytes()
        };
        let xmp = packet("old");
        let mut resource = b"8BIM".to_vec();
        resource.extend_from_slice(&0x0424_u16.to_be_bytes());
        resource.extend_from_slice(&[0, 0]);
        resource.extend_from_slice(&(xmp.len() as u32).to_be_bytes());
        resource.extend_from_slice(&xmp);
        if xmp.len() % 2 == 1 {
            resource.push(0);
        }
        let mut bytes = vec![0_u8; 26];
        bytes[..4].copy_from_slice(b"8BPS");
        bytes[4..6].copy_from_slice(&1_u16.to_be_bytes());
        bytes[12..14].copy_from_slice(&3_u16.to_be_bytes());
        bytes[14..18].copy_from_slice(&100_u32.to_be_bytes());
        bytes[18..22].copy_from_slice(&200_u32.to_be_bytes());
        bytes[22..24].copy_from_slice(&8_u16.to_be_bytes());
        bytes[24..26].copy_from_slice(&3_u16.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&(resource.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&resource);
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&0_u16.to_be_bytes());

        let replacement = String::from_utf8(packet("new")).expect("XMP fixture should be UTF-8");
        let handler = handler_for_format(FileFormat::Psd).expect("PSD handler should exist");
        let mut reader = std::io::Cursor::new(bytes.clone());
        let mut writer = std::io::Cursor::new(Vec::new());
        handler
            .write_metadata(
                &mut reader,
                &mut writer,
                FileInfo::new("handler.psd".into(), bytes.len() as u64, FileFormat::Psd),
                ParseLimits::default(),
                &[MetadataEdit::set("PSD:XMP", replacement)],
            )
            .expect("registered PSD writer should accept canonical edits");

        let output = writer.into_inner();
        let metadata = crate::read_reader(
            &mut std::io::Cursor::new(output.clone()),
            FileInfo::new(
                "handler.psd".into(),
                output.len() as u64,
                FileFormat::Unknown,
            ),
        )
        .expect("registered PSD writer output should remain readable");
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn handlers_without_writers_report_explicit_unsupported_errors() {
        let handler = handler_for_format(FileFormat::Mkv).expect("MKV handler should exist");
        let mut reader = std::io::Cursor::new(b"\x1A\x45\xDF\xA3".to_vec());
        let mut writer = std::io::Cursor::new(Vec::new());
        let error = handler
            .write_metadata(
                &mut reader,
                &mut writer,
                FileInfo::new("document.mkv".into(), 4, FileFormat::Mkv),
                ParseLimits::default(),
                &[MetadataEdit::set("PDF:Title", "new")],
            )
            .expect_err("MKV has no validated writer");
        assert!(error.to_string().contains("MKV"));
    }
}

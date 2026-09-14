use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType,
};

const TIFF_LITTLE_ENDIAN: &[u8; 4] = b"II*\0";
const TIFF_BIG_ENDIAN: &[u8; 4] = b"MM\0*";
const RW2_LITTLE_ENDIAN: &[u8; 4] = b"IIU\0";
const RW2_BIG_ENDIAN: &[u8; 4] = b"MM\0U";
const BIG_TIFF_LITTLE_ENDIAN: &[u8; 4] = b"II+\0";
const BIG_TIFF_BIG_ENDIAN: &[u8; 4] = b"MM\0+";
const RAF_SIGNATURE: &[u8; 16] = b"FUJIFILMCCD-RAW ";
const MRW_SIGNATURE: &[u8; 4] = b"\0MRM";
const X3F_SIGNATURE: &[u8; 4] = b"FOVb";

pub fn read_raw<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    if file_length < 4 {
        return Err(MetraError::InvalidHeader {
            context: "RAW".to_owned(),
            message: "file is shorter than a RAW signature".to_owned(),
        });
    }
    let prefix_length =
        usize::try_from(file_length.min(16)).map_err(|_| MetraError::InvalidOffset {
            context: "RAW signature".to_owned(),
            offset: file_length,
        })?;
    let prefix = read_at(
        reader,
        0,
        prefix_length,
        file_length,
        &path,
        "RAW signature",
    )?;

    if is_tiff_header(&prefix) {
        let mut metadata = crate::tiff::read_tiff(reader, file_info, limits)?;
        add_identity(&mut metadata, raw_variant(&path).unwrap_or("TIFF-like RAW"));
        metadata.sort_tags();
        Ok(metadata)
    } else if is_cr3_header(&prefix) {
        let mut metadata = crate::isobmff::read_isobmff(reader, file_info, limits)?;
        add_identity(&mut metadata, "CR3");
        metadata.sort_tags();
        Ok(metadata)
    } else if prefix.starts_with(RAF_SIGNATURE) {
        crate::raf::read_raf(reader, file_info, limits)
    } else if is_crw_header(&prefix) {
        crate::crw::read_crw(reader, file_info, limits)
    } else if prefix.starts_with(MRW_SIGNATURE) {
        crate::mrw::read_mrw(reader, file_info, limits)
    } else if prefix.starts_with(X3F_SIGNATURE) {
        crate::x3f::read_x3f(reader, file_info, limits)
    } else {
        Err(MetraError::InvalidHeader {
            context: "RAW".to_owned(),
            message: "unsupported RAW container signature".to_owned(),
        })
    }
}

pub(crate) fn is_tiff_header(bytes: &[u8]) -> bool {
    bytes.starts_with(TIFF_LITTLE_ENDIAN)
        || bytes.starts_with(TIFF_BIG_ENDIAN)
        || bytes.starts_with(RW2_LITTLE_ENDIAN)
        || bytes.starts_with(RW2_BIG_ENDIAN)
        || bytes.starts_with(BIG_TIFF_LITTLE_ENDIAN)
        || bytes.starts_with(BIG_TIFF_BIG_ENDIAN)
}

pub(crate) fn is_crw_header(bytes: &[u8]) -> bool {
    bytes.len() >= 14 && matches!(&bytes[..2], b"II" | b"MM") && &bytes[6..14] == b"HEAPCCDR"
}

pub(crate) fn is_mrw_header(bytes: &[u8]) -> bool {
    bytes.starts_with(MRW_SIGNATURE)
}

pub(crate) fn is_x3f_header(bytes: &[u8]) -> bool {
    bytes.starts_with(X3F_SIGNATURE)
}

pub(crate) fn is_cr3_header(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"crx " | b"CRX ")
}

pub(crate) fn is_cr2_header(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && is_tiff_header(bytes) && &bytes[8..12] == b"CR\x02\0"
}

pub(crate) fn is_raf_header(bytes: &[u8]) -> bool {
    bytes.starts_with(RAF_SIGNATURE)
}

pub(crate) fn raw_variant(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "dng" => Some("DNG"),
        "cr2" => Some("CR2"),
        "cr3" => Some("CR3"),
        "nef" => Some("NEF"),
        "arw" => Some("ARW"),
        "raf" => Some("RAF"),
        "orf" => Some("ORF"),
        "rw2" => Some("RW2"),
        "pef" => Some("PEF"),
        "crw" => Some("CRW"),
        "mrw" => Some("MRW"),
        "x3f" => Some("X3F"),
        "raw" => Some("RAW"),
        _ => None,
    }
}

pub(crate) fn add_identity(metadata: &mut Metadata, variant: &str) {
    add_tag(
        metadata,
        "Container",
        TagValue::String("RAW".to_owned()),
        Source::new("RAW/header", Some(0), None),
    );
    add_tag(
        metadata,
        "Variant",
        TagValue::String(variant.to_owned()),
        Source::new("RAW/header", Some(0), None),
    );
}

fn add_tag(metadata: &mut Metadata, name: &str, value: TagValue, source: Source) {
    metadata.add_tag(Tag {
        namespace: "RAW".to_owned(),
        group: "Container".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("RAW container identity".to_owned()),
        raw_value: None,
        value,
        value_type: ValueType::String,
        source,
        writable: false,
    });
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    file_length: u64,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    let length_u64 = u64::try_from(length).map_err(|_| MetraError::InvalidOffset {
        context: context.to_owned(),
        offset,
    })?;
    let end = offset
        .checked_add(length_u64)
        .ok_or(MetraError::InvalidOffset {
            context: context.to_owned(),
            offset,
        })?;
    if end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: context.to_owned(),
        });
    }
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| MetraError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn file_info(name: &str, bytes: &[u8]) -> FileInfo {
        FileInfo::new(name.into(), bytes.len() as u64, metra_core::FileFormat::Raw)
    }

    fn minimal_tiff() -> Vec<u8> {
        vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    }

    fn minimal_rw2() -> Vec<u8> {
        let mut bytes = vec![b'I', b'I', b'U', 0, 8, 0, 0, 0, 1, 0];
        bytes.extend_from_slice(&[0x0F, 0x01, 2, 0, 5, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(b"RW2\0\0");
        bytes
    }

    #[test]
    fn delegates_tiff_like_raw_to_exif_reader() {
        let bytes = minimal_tiff();
        let metadata = read_raw(
            &mut Cursor::new(bytes.clone()),
            file_info("capture.dng", &bytes),
            ParseLimits::default(),
        )
        .expect("DNG-like TIFF should parse");
        assert_eq!(metadata.file_info.format, metra_core::FileFormat::Raw);
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "DNG");
    }

    #[test]
    fn delegates_rw2_tiff_dialect_to_exif_reader() {
        let bytes = minimal_rw2();
        let metadata = read_raw(
            &mut Cursor::new(bytes.clone()),
            file_info("capture.rw2", &bytes),
            ParseLimits::default(),
        )
        .expect("RW2 TIFF dialect should parse");
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "RW2");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "RW2");
    }

    #[test]
    fn identifies_cr3_without_decoding_media_payloads() {
        let bytes = b"\0\0\0\0ftypcrx \0\0\0\0".to_vec();
        let metadata = read_raw(
            &mut Cursor::new(bytes.clone()),
            file_info("capture.cr3", &bytes),
            ParseLimits::default(),
        )
        .expect("CR3 header should delegate to ISO-BMFF reader");
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CR3");
    }

    #[test]
    fn reports_raf_as_identified_but_partially_decoded() {
        let bytes = RAF_SIGNATURE.to_vec();
        let metadata = read_raw(
            &mut Cursor::new(bytes.clone()),
            file_info("capture.raf", &bytes),
            ParseLimits::default(),
        )
        .expect("RAF signature should be identified");
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "RAF");
        assert!(
            metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "raw-raf-partial")
        );
    }

    #[test]
    fn identifies_legacy_raw_signatures_with_explicit_partial_warnings() {
        for (name, bytes, variant, warning) in [
            (
                "capture.crw",
                b"II\x1A\0\0\0HEAPCCDR\0\0\0\0".as_slice(),
                "CRW",
                "raw-crw-partial",
            ),
            (
                "capture.mrw",
                b"\0MRM\0\0\0\0".as_slice(),
                "MRW",
                "raw-mrw-partial",
            ),
            (
                "capture.x3f",
                b"FOVb\0\0\0\0".as_slice(),
                "X3F",
                "raw-x3f-partial",
            ),
        ] {
            let metadata = read_raw(
                &mut Cursor::new(bytes.to_vec()),
                file_info(name, bytes),
                ParseLimits::default(),
            )
            .expect("known legacy RAW signature should be identified");
            assert_eq!(
                metadata.find("RAW:Variant").unwrap().display_value(),
                variant
            );
            assert!(
                metadata
                    .warnings
                    .iter()
                    .any(|warning_item| warning_item.code == warning)
            );
        }
    }
}

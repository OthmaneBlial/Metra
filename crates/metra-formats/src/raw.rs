use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const TIFF_LITTLE_ENDIAN: &[u8; 4] = b"II*\0";
const TIFF_BIG_ENDIAN: &[u8; 4] = b"MM\0*";
const RAF_SIGNATURE: &[u8; 16] = b"FUJIFILMCCD-RAW ";

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
        let mut metadata = Metadata::new(file_info);
        add_identity(&mut metadata, "RAF");
        metadata.add_warning(
            Warning::new(
                "raw-raf-partial",
                "RAF container identified; Fuji-specific metadata and image payload are not decoded",
            )
            .at(0),
        );
        Ok(metadata)
    } else {
        Err(MetraError::InvalidHeader {
            context: "RAW".to_owned(),
            message: "unsupported RAW container signature".to_owned(),
        })
    }
}

pub(crate) fn is_tiff_header(bytes: &[u8]) -> bool {
    bytes.starts_with(TIFF_LITTLE_ENDIAN) || bytes.starts_with(TIFF_BIG_ENDIAN)
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
        "raw" => Some("RAW"),
        _ => None,
    }
}

fn add_identity(metadata: &mut Metadata, variant: &str) {
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
}

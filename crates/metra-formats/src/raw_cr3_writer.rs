use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::isobmff_writer::{IsobmffEdit, rewrite_isobmff};
use crate::raw::read_raw;

/// Rewrites existing ISO-BMFF text items in a Canon CR3 container.
///
/// CR3 is reported as RAW by the public detector, but its metadata container
/// is ISO-BMFF. This adapter requires the RAW reader to identify CR3 before it
/// delegates the fixed-box rewrite to the ISO-BMFF writer.
pub fn rewrite_raw_cr3<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Raw {
        return Err(MetraError::WriteFailure {
            message: format!("CR3 writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_raw(reader, file_info.clone(), limits)?;
    ensure_cr3(&metadata)?;
    let iso_file_info = FileInfo::new(file_info.path, file_info.size, FileFormat::Mp4);
    rewrite_isobmff(reader, writer, iso_file_info, limits, edits)
}

pub fn rewrite_raw_cr3_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_raw_cr3(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_raw(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Raw),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_raw_cr3_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[IsobmffEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = crate::read_path_with_limits(&path, limits)?;
    ensure_cr3(&source_metadata)?;
    let source_file_metadata = fs::metadata(&path).map_err(|source| MetraError::Io {
        path: path.clone(),
        source,
    })?;
    let file_info = FileInfo::new(path.clone(), source_file_metadata.len(), FileFormat::Raw);
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut input = File::open(&path).map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot create {}: {source}", temp_path.display()),
            })?;
        rewrite_raw_cr3(&mut input, &mut output, file_info.clone(), limits, edits)?;
        output
            .sync_all()
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot sync {}: {source}", temp_path.display()),
            })?;
        drop(output);

        let mut validation = File::open(&temp_path).map_err(|source| MetraError::Io {
            path: temp_path.clone(),
            source,
        })?;
        let written_size = validation
            .metadata()
            .map_err(|source| MetraError::Io {
                path: temp_path.clone(),
                source,
            })?
            .len();
        read_raw(
            &mut validation,
            FileInfo::new(temp_path.clone(), written_size, FileFormat::Raw),
            limits,
        )?;
        fs::set_permissions(&temp_path, source_file_metadata.permissions()).map_err(|source| {
            MetraError::WriteFailure {
                message: format!(
                    "cannot preserve permissions on {}: {source}",
                    temp_path.display()
                ),
            }
        })?;
        atomic_replace(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
            message: format!("cannot atomically replace {}: {source}", path.display()),
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn ensure_cr3(metadata: &metra_core::Metadata) -> Result<()> {
    let Some(tag) = metadata.find("RAW:Variant") else {
        return Err(MetraError::WriteFailure {
            message: "RAW container has no identified variant".to_owned(),
        });
    };
    if matches!(&tag.value, metra_core::TagValue::String(variant) if variant == "CR3") {
        Ok(())
    } else {
        Err(MetraError::UnsupportedFormat {
            description: "validated RAW ISO-BMFF writing requires a CR3 container".to_owned(),
        })
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.cr3");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| MetraError::WriteFailure {
            message: format!("cannot create temporary name: {error}"),
        })?
        .as_nanos();
    Ok(path.with_file_name(format!(
        ".{filename}.metra-{}-{timestamp}.tmp",
        std::process::id()
    )))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn box_with_kind(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let size = u32::try_from(data.len() + 8).expect("test box fits");
        let mut bytes = size.to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);
        bytes
    }

    fn minimal_cr3(title: &str) -> Vec<u8> {
        let ftyp = box_with_kind(b"ftyp", b"crx \0\0\0\0crx ");
        let data = box_with_kind(
            b"data",
            &[&[0, 0, 0, 1, 0, 0, 0, 0], title.as_bytes()].concat(),
        );
        let title_kind = [0xA9, b'n', b'a', b'm'];
        let title = box_with_kind(&title_kind, &data);
        let ilst = box_with_kind(b"ilst", &title);
        let udta = box_with_kind(b"udta", &ilst);
        let moov = box_with_kind(b"moov", &udta);
        [ftyp, moov].concat()
    }

    #[test]
    fn rewrites_existing_cr3_isobmff_text_without_changing_layout() {
        let bytes = minimal_cr3("old");
        let output = rewrite_raw_cr3_to_vec(
            &bytes,
            FileInfo::new("capture.cr3".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
            &[IsobmffEdit::SetText {
                key: "ISOBMFF:Title".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .expect("CR3 ISO-BMFF rewrite should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_raw(
            &mut Cursor::new(output),
            FileInfo::new("capture.cr3".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .expect("rewritten CR3 should remain readable");
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "CR3");
        assert_eq!(
            metadata.find("ISOBMFF:Title").unwrap().display_value(),
            "new"
        );
    }

    #[test]
    fn rejects_non_cr3_raw_before_delegating_to_isobmff() {
        let bytes = b"FUJIFILMCCD-RAW ".to_vec();
        let error = rewrite_raw_cr3_to_vec(
            &bytes,
            FileInfo::new("capture.raf".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
            &[IsobmffEdit::SetText {
                key: "ISOBMFF:Title".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .expect_err("RAF must not enter the CR3 writer");
        assert!(error.to_string().contains("CR3"));
    }

    #[test]
    fn path_rewrite_is_atomic_and_leaves_source_on_rejected_growth() {
        let bytes = minimal_cr3("old");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("metra-cr3-path-{nonce}.cr3"));
        fs::write(&path, &bytes).expect("CR3 fixture should be writable");

        rewrite_raw_cr3_path(
            &path,
            ParseLimits::default(),
            &[IsobmffEdit::SetText {
                key: "ISOBMFF:Title".to_owned(),
                value: "new".to_owned(),
            }],
        )
        .expect("CR3 path rewrite should succeed");
        let rewritten = fs::read(&path).expect("rewritten CR3 should be readable");
        assert_eq!(rewritten.len(), bytes.len());
        let metadata = read_raw(
            &mut Cursor::new(rewritten.clone()),
            FileInfo::new("path.cr3".into(), rewritten.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .expect("rewritten CR3 should validate");
        assert_eq!(
            metadata.find("ISOBMFF:Title").unwrap().display_value(),
            "new"
        );

        let error = rewrite_raw_cr3_path(
            &path,
            ParseLimits::default(),
            &[IsobmffEdit::SetText {
                key: "ISOBMFF:Title".to_owned(),
                value: "value is too long".to_owned(),
            }],
        )
        .expect_err("growth beyond the existing CR3 slot should be rejected");
        assert!(error.to_string().contains("needs"));
        assert_eq!(
            fs::read(&path).expect("source should remain present"),
            rewritten
        );
        fs::remove_file(&path).expect("test CR3 should be removable");
    }
}

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::raw::read_raw;
use crate::tiff_writer::{TiffEdit, rewrite_tiff};

/// Rewrites existing TIFF/BigTIFF ASCII slots in TIFF-like RAW containers.
///
/// DNG, CR2, NEF, ARW, ORF, RW2, and PEF payloads retain their original bytes
/// outside the selected TIFF value slot. Proprietary RAW containers such as
/// CR3, RAF, CRW, MRW, and X3F are rejected by this adapter.
pub fn rewrite_raw_tiff<R: Read + Seek, W: Write + Seek>(
    reader: &mut R,
    writer: &mut W,
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<()> {
    if file_info.format != FileFormat::Raw {
        return Err(MetraError::WriteFailure {
            message: format!("RAW TIFF writer cannot edit {}", file_info.format),
        });
    }
    let metadata = read_raw(reader, file_info.clone(), limits)?;
    ensure_tiff_like(&metadata)?;
    rewrite_tiff(reader, writer, file_info, limits, edits)
}

pub fn rewrite_raw_tiff_to_vec(
    bytes: &[u8],
    file_info: FileInfo,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<Vec<u8>> {
    let mut reader = std::io::Cursor::new(bytes);
    let mut output = std::io::Cursor::new(Vec::new());
    rewrite_raw_tiff(&mut reader, &mut output, file_info.clone(), limits, edits)?;
    let output = output.into_inner();
    read_raw(
        &mut std::io::Cursor::new(output.as_slice()),
        FileInfo::new(file_info.path, output.len() as u64, FileFormat::Raw),
        limits,
    )?;
    Ok(output)
}

pub fn rewrite_raw_tiff_path(
    path: impl AsRef<Path>,
    limits: ParseLimits,
    edits: &[TiffEdit],
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    let source_metadata = crate::read_path_with_limits(&path, limits)?;
    if source_metadata.file_info.format != FileFormat::Raw {
        return Err(MetraError::WriteFailure {
            message: format!(
                "RAW TIFF writer cannot edit {}",
                source_metadata.file_info.format
            ),
        });
    }
    ensure_tiff_like(&source_metadata)?;
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
        rewrite_raw_tiff(&mut input, &mut output, file_info.clone(), limits, edits)?;
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
        drop(validation);
        drop(input);
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

fn ensure_tiff_like(metadata: &metra_core::Metadata) -> Result<()> {
    let Some(tag) = metadata.find("RAW:Variant") else {
        return Err(MetraError::WriteFailure {
            message: "RAW container has no identified variant".to_owned(),
        });
    };
    let metra_core::TagValue::String(variant) = &tag.value else {
        return Err(MetraError::WriteFailure {
            message: "RAW variant is not a string".to_owned(),
        });
    };
    if matches!(
        variant.as_str(),
        "DNG" | "CR2" | "NEF" | "ARW" | "ORF" | "RW2" | "PEF" | "RAW" | "TIFF-like RAW"
    ) {
        Ok(())
    } else {
        Err(MetraError::UnsupportedFormat {
            description: format!(
                "validated RAW writing is not implemented for {variant}; only TIFF-like RAW containers are supported"
            ),
        })
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.raw");
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

    fn dng_with_make(make: &[u8]) -> Vec<u8> {
        let mut bytes = vec![
            b'I',
            b'I',
            42,
            0,
            8,
            0,
            0,
            0,
            1,
            0, // one IFD0 entry
            0x0F,
            0x01,
            2,
            0,
            (make.len() & 0xFF) as u8,
            ((make.len() >> 8) & 0xFF) as u8,
            0,
            0,
            26,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // no next IFD
        ];
        assert_eq!(bytes.len(), 26);
        bytes.extend_from_slice(make);
        bytes
    }

    #[test]
    fn rewrites_tiff_ascii_in_a_dng_without_changing_layout() {
        let bytes = dng_with_make(b"Canon\0");
        let output = rewrite_raw_tiff_to_vec(
            &bytes,
            FileInfo::new("capture.dng".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .expect("DNG TIFF-like rewrite should succeed");
        assert_eq!(output.len(), bytes.len());
        let metadata = read_raw(
            &mut Cursor::new(output),
            FileInfo::new("capture.dng".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
        )
        .expect("rewritten DNG should remain readable");
        assert_eq!(metadata.find("EXIF:Make").unwrap().display_value(), "Sony");
        assert_eq!(metadata.find("RAW:Variant").unwrap().display_value(), "DNG");
    }

    #[test]
    fn rejects_non_tiff_like_raw_variants_before_writing() {
        let bytes = b"FUJIFILMCCD-RAW ".to_vec();
        let error = rewrite_raw_tiff_to_vec(
            &bytes,
            FileInfo::new("capture.raf".into(), bytes.len() as u64, FileFormat::Raw),
            ParseLimits::default(),
            &[TiffEdit::SetAscii {
                key: "EXIF:Make".to_owned(),
                value: "Sony".to_owned(),
            }],
        )
        .unwrap_err();
        assert!(error.to_string().contains("TIFF-like"));
    }
}

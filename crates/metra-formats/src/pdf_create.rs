use std::fmt::Write as FmtWrite;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::pdf::read_pdf;

/// One bounded PDF Info field for a newly created document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfCreateEntry {
    pub name: String,
    pub value: String,
}

impl PdfCreateEntry {
    pub fn info(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal PDF document with an Info dictionary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PdfCreateOptions {
    pub info: Vec<PdfCreateEntry>,
}

impl PdfCreateOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_info(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.info.push(PdfCreateEntry::info(name, value));
        self
    }

    pub fn push_info(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.info.push(PdfCreateEntry::info(name, value));
    }
}

/// Create a valid minimal PDF container with a calculated xref table.
pub fn create_pdf_to_vec(options: &PdfCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.info.len() > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "PDF creation Info entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }

    let mut fields: Vec<(&str, &str)> = Vec::with_capacity(options.info.len());
    for entry in &options.info {
        let name = canonical_info_name(&entry.name).ok_or_else(|| MetraError::InvalidTag {
            context: "PDF creation".to_owned(),
            message: format!("unsupported Info field {}", entry.name),
        })?;
        if entry.value.contains('\0') {
            return Err(MetraError::InvalidTag {
                context: format!("PDF creation {name}"),
                message: "values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("PDF creation value {name}"),
                limit: limits.max_value_bytes,
            });
        }
        if fields.iter().any(|existing| existing.0 == name) {
            return Err(MetraError::InvalidTag {
                context: "PDF creation".to_owned(),
                message: format!("duplicate Info field {name}"),
            });
        }
        fields.push((name, entry.value.as_str()));
    }

    let mut body = Vec::new();
    body.extend_from_slice(b"%PDF-1.7\n%\xFF\xFF\xFF\xFF\n");
    let mut offsets = vec![0_usize];
    append_object(
        &mut body,
        &mut offsets,
        b"<< /Type /Catalog /Pages 2 0 R >>",
    );
    append_object(&mut body, &mut offsets, b"<< /Type /Pages /Count 0 >>");

    let mut info = String::from("<<");
    for (name, value) in fields {
        write!(&mut info, " /{name} ").map_err(|_| write_failure("PDF Info dictionary"))?;
        append_utf16_hex(&mut info, value)?;
    }
    info.push_str(" >>");
    append_object(&mut body, &mut offsets, info.as_bytes());

    let xref_offset = body.len();
    let object_count = offsets.len();
    writeln!(&mut body, "xref").map_err(|_| write_failure("PDF xref"))?;
    writeln!(&mut body, "0 {object_count}").map_err(|_| write_failure("PDF xref"))?;
    body.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        let offset = u64::try_from(*offset).map_err(|_| size_error("PDF xref offset"))?;
        writeln!(&mut body, "{offset:010} 00000 n ")
            .map_err(|_| write_failure("PDF xref entry"))?;
    }
    writeln!(&mut body, "trailer").map_err(|_| write_failure("PDF trailer"))?;
    writeln!(
        &mut body,
        "<< /Size {object_count} /Root 1 0 R /Info 3 0 R >>"
    )
    .map_err(|_| write_failure("PDF trailer"))?;
    writeln!(&mut body, "startxref").map_err(|_| write_failure("PDF startxref"))?;
    writeln!(&mut body, "{xref_offset}").map_err(|_| write_failure("PDF startxref"))?;
    body.extend_from_slice(b"%%EOF\n");

    if body.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "PDF creation file".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    read_pdf(
        &mut std::io::Cursor::new(body.as_slice()),
        FileInfo::new("created.pdf".into(), body.len() as u64, FileFormat::Pdf),
        limits,
    )?;
    Ok(body)
}

/// Create a new PDF document without overwriting an existing path.
pub fn create_pdf_path(
    path: impl AsRef<Path>,
    options: &PdfCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing PDF {}", path.display()),
        });
    }
    let bytes = create_pdf_to_vec(options, limits)?;
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| write_error(&temp_path, source))?;
        output
            .write_all(&bytes)
            .map_err(|source| write_error(&temp_path, source))?;
        output
            .sync_all()
            .map_err(|source| write_error(&temp_path, source))?;
        drop(output);
        read_pdf(
            &mut std::io::Cursor::new(bytes.as_slice()),
            FileInfo::new(temp_path.clone(), bytes.len() as u64, FileFormat::Pdf),
            limits,
        )?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing PDF {}", path.display()),
            });
        }
        atomic_replace(&temp_path, &path).map_err(|source| MetraError::WriteFailure {
            message: format!("cannot atomically create {}: {source}", path.display()),
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn canonical_info_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "Title" => "Title",
        "Author" => "Author",
        "Subject" => "Subject",
        "Keywords" => "Keywords",
        "Creator" => "Creator",
        "Producer" => "Producer",
        "CreationDate" => "CreationDate",
        "ModifyDate" | "ModDate" => "ModDate",
        "Trapped" => "Trapped",
        _ => return None,
    })
}

fn append_object(body: &mut Vec<u8>, offsets: &mut Vec<usize>, dictionary: &[u8]) {
    offsets.push(body.len());
    let number = offsets.len() - 1;
    let _ = writeln!(body, "{number} 0 obj");
    body.extend_from_slice(dictionary);
    body.extend_from_slice(b"\nendobj\n");
}

fn append_utf16_hex(output: &mut String, value: &str) -> Result<()> {
    output.push_str("<FEFF");
    for unit in value.encode_utf16() {
        write!(output, "{unit:04X}").map_err(|_| write_failure("PDF UTF-16 string"))?;
    }
    output.push('>');
    Ok(())
}

fn size_error(resource: &str) -> MetraError {
    MetraError::ResourceLimitExceeded {
        resource: resource.to_owned(),
        limit: usize::MAX,
    }
}

fn write_failure(resource: &str) -> MetraError {
    MetraError::WriteFailure {
        message: format!("cannot construct {resource}"),
    }
}

fn write_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("cannot write {}: {source}", path.display()),
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("metadata.pdf");
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

    #[test]
    fn creates_readable_pdf_with_unicode_info() {
        let bytes = create_pdf_to_vec(
            &PdfCreateOptions::new()
                .with_info("Title", "Metra résumé")
                .with_info("Author", "Othmane"),
            ParseLimits::default(),
        )
        .unwrap();
        let metadata = read_pdf(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("created.pdf".into(), bytes.len() as u64, FileFormat::Pdf),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("PDF:Title").unwrap().display_value(),
            "Metra résumé"
        );
        assert_eq!(
            metadata.find("PDF:Author").unwrap().display_value(),
            "Othmane"
        );
        assert!(bytes.windows(4).any(|window| window == b"xref"));
    }

    #[test]
    fn rejects_invalid_duplicate_and_oversized_info() {
        let invalid = PdfCreateOptions::new().with_info("Nope", "value");
        assert!(create_pdf_to_vec(&invalid, ParseLimits::default()).is_err());
        let duplicate = PdfCreateOptions::new()
            .with_info("Title", "one")
            .with_info("Title", "two");
        assert!(create_pdf_to_vec(&duplicate, ParseLimits::default()).is_err());
        let limits = ParseLimits {
            max_metadata_bytes: 10,
            ..ParseLimits::default()
        };
        assert!(create_pdf_to_vec(&PdfCreateOptions::new(), limits).is_err());
    }

    #[test]
    fn path_creation_refuses_overwrite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-pdf-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("seed.pdf");
        create_pdf_path(
            &path,
            &PdfCreateOptions::new().with_info("Title", "Metra"),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(create_pdf_path(&path, &PdfCreateOptions::new(), ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}

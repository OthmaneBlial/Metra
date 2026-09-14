use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::svg::read_svg;

/// Options for creating a minimal SVG metadata seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SvgCreateOptions {
    pub title: Option<String>,
    pub description: Option<String>,
    pub comments: Vec<String>,
}

impl SvgCreateOptions {
    /// Start with an empty 1x1 SVG document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the document title and return the updated options.
    pub fn with_title(mut self, value: impl Into<String>) -> Self {
        self.title = Some(value.into());
        self
    }

    /// Set the document description and return the updated options.
    pub fn with_description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// Add a document comment and return the updated options.
    pub fn with_comment(mut self, value: impl Into<String>) -> Self {
        self.comments.push(value.into());
        self
    }

    /// Set the document title in place.
    pub fn set_title(&mut self, value: impl Into<String>) {
        self.title = Some(value.into());
    }

    /// Set the document description in place.
    pub fn set_description(&mut self, value: impl Into<String>) {
        self.description = Some(value.into());
    }

    /// Add a document comment in place.
    pub fn push_comment(&mut self, value: impl Into<String>) {
        self.comments.push(value.into());
    }
}

/// Create a minimal 1x1 SVG metadata seed in memory.
pub fn create_svg_to_vec(options: &SvgCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.comments.len() > limits.max_jpeg_segments {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "SVG creation comments".to_owned(),
            limit: limits.max_jpeg_segments,
        });
    }
    let title = validate_text(options.title.as_deref(), "SVG creation title", limits)?;
    let description = validate_text(
        options.description.as_deref(),
        "SVG creation description",
        limits,
    )?;
    let comments = options
        .comments
        .iter()
        .map(|value| validate_comment(value, limits))
        .collect::<Result<Vec<_>>>()?;

    let mut document = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\" viewBox=\"0 0 1 1\">",
    );
    if let Some(title) = title {
        document.push_str("<title>");
        document.push_str(&escape_xml(&title));
        document.push_str("</title>");
    }
    if let Some(description) = description {
        document.push_str("<desc>");
        document.push_str(&escape_xml(&description));
        document.push_str("</desc>");
    }
    for comment in comments {
        document.push_str("<!--");
        document.push_str(&comment);
        document.push_str("-->");
    }
    document.push_str("</svg>\n");
    let bytes = document.into_bytes();
    if bytes.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "SVG creation document".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_svg(&bytes, limits)?;
    Ok(bytes)
}

/// Create a new SVG metadata seed without overwriting an existing path.
pub fn create_svg_path(
    path: impl AsRef<Path>,
    options: &SvgCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing SVG {}", path.display()),
        });
    }
    let bytes = create_svg_to_vec(options, limits)?;
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
        validate_created_svg(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing SVG {}", path.display()),
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

fn validate_text(
    value: Option<&str>,
    resource: &str,
    limits: ParseLimits,
) -> Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: resource.to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    if !value.chars().all(valid_xml_char) {
        return Err(MetraError::InvalidXml {
            message: format!("{resource} contains characters forbidden by XML 1.0"),
        });
    }
    Ok(Some(value.to_owned()))
}

fn validate_comment(value: &str, limits: ParseLimits) -> Result<String> {
    if value.len() > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "SVG creation comment".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    if value.contains("--") || value.ends_with('-') {
        return Err(MetraError::InvalidXml {
            message: "SVG comments cannot contain '--' or end with '-'".to_owned(),
        });
    }
    if !value.chars().all(valid_xml_char) {
        return Err(MetraError::InvalidXml {
            message: "SVG creation comment contains characters forbidden by XML 1.0".to_owned(),
        });
    }
    Ok(value.to_owned())
}

fn valid_xml_char(character: char) -> bool {
    matches!(character, '\u{9}' | '\u{A}' | '\u{D}')
        || ('\u{20}'..='\u{D7FF}').contains(&character)
        || ('\u{E000}'..='\u{FFFD}').contains(&character)
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            character => escaped.push(character),
        }
    }
    escaped
}

fn validate_created_svg(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_svg(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.svg".into(), bytes.len() as u64, FileFormat::Svg),
        limits,
    )?;
    Ok(())
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
        .unwrap_or("metadata.svg");
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
    fn creates_readable_svg_text_and_escaped_values() {
        let options = SvgCreateOptions::new()
            .with_title("Metra & review")
            .with_description("<bounded>")
            .with_comment("Othmane review");
        let bytes = create_svg_to_vec(&options, ParseLimits::default()).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("&amp;"));
        let metadata = read_svg(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("created.svg".into(), bytes.len() as u64, FileFormat::Svg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(
            metadata.find("SVG:Title").unwrap().display_value(),
            "Metra & review"
        );
        assert_eq!(
            metadata.find("SVG:Description").unwrap().display_value(),
            "<bounded>"
        );
        assert_eq!(
            metadata.find("SVG:Comment").unwrap().display_value(),
            "Othmane review"
        );
    }

    #[test]
    fn rejects_invalid_comments_and_values() {
        let invalid_comment = SvgCreateOptions::new().with_comment("unsafe--comment");
        assert!(create_svg_to_vec(&invalid_comment, ParseLimits::default()).is_err());

        let invalid_text = SvgCreateOptions::new().with_title("bad\u{1}");
        assert!(create_svg_to_vec(&invalid_text, ParseLimits::default()).is_err());
    }

    #[test]
    fn path_creation_refuses_overwrite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-svg-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("seed.svg");
        create_svg_path(
            &path,
            &SvgCreateOptions::new().with_title("created"),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(create_svg_path(&path, &SvgCreateOptions::new(), ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}

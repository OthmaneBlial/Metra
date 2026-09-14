use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::ogg::{ogg_crc, read_ogg};

const OGG_HEADER_LENGTH: usize = 27;
const VENDOR: &[u8] = b"Metra";
const SERIAL: u32 = 0x4D45_5452;
const MAX_SEGMENTS_PER_PAGE: usize = 255;
const MAX_FULL_SEGMENTS_WITH_TERMINATOR: usize = MAX_SEGMENTS_PER_PAGE - 1;

/// One bounded UTF-8 Vorbis-style comment for a new Ogg Opus seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OggCreateEntry {
    pub key: String,
    pub value: String,
}

impl OggCreateEntry {
    /// Create one comment. The key and value are validated when encoded.
    pub fn comment(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Options for creating a minimal Ogg Opus metadata seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OggCreateOptions {
    pub comments: Vec<OggCreateEntry>,
}

impl OggCreateOptions {
    /// Start with an empty OpusTags packet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one comment and return the updated options.
    pub fn with_comment(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.comments.push(OggCreateEntry::comment(key, value));
        self
    }

    /// Add one comment in place.
    pub fn push_comment(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.comments.push(OggCreateEntry::comment(key, value));
    }
}

/// Create a minimal Ogg Opus metadata seed in memory.
pub fn create_ogg_to_vec(options: &OggCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    if options.comments.len() > limits.max_jpeg_segments {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Ogg creation comment entries".to_owned(),
            limit: limits.max_jpeg_segments,
        });
    }

    let mut comments = Vec::with_capacity(options.comments.len());
    let mut packet_size = 8_usize
        .checked_add(4)
        .and_then(|size| size.checked_add(VENDOR.len()))
        .and_then(|size| size.checked_add(4))
        .ok_or_else(|| size_error("Ogg creation comment packet"))?;
    for entry in &options.comments {
        let key = validate_key(&entry.key)?;
        if entry.value.contains('\0') {
            return Err(MetraError::InvalidTag {
                context: format!("Ogg creation {}", entry.key),
                message: "comment values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("Ogg creation value {}", entry.key),
                limit: limits.max_value_bytes,
            });
        }
        if comments
            .iter()
            .any(|existing: &(Vec<u8>, String)| existing.0 == key)
        {
            return Err(MetraError::InvalidTag {
                context: "Ogg creation".to_owned(),
                message: format!("duplicate Ogg comment key {}", entry.key),
            });
        }
        let value_size = key
            .len()
            .checked_add(1)
            .and_then(|size| size.checked_add(entry.value.len()))
            .ok_or_else(|| size_error("Ogg creation comment"))?;
        u32::try_from(value_size).map_err(|_| size_error("Ogg creation comment"))?;
        packet_size = packet_size
            .checked_add(4)
            .and_then(|size| size.checked_add(value_size))
            .ok_or_else(|| size_error("Ogg creation comment packet"))?;
        comments.push((key, entry.value.clone()));
    }
    if packet_size > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Ogg creation comment packet".to_owned(),
            limit: limits.max_value_bytes,
        });
    }

    let mut tags = Vec::with_capacity(packet_size);
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(
        &u32::try_from(VENDOR.len())
            .map_err(|_| size_error("Ogg creation vendor"))?
            .to_le_bytes(),
    );
    tags.extend_from_slice(VENDOR);
    tags.extend_from_slice(
        &u32::try_from(comments.len())
            .map_err(|_| size_error("Ogg creation comment count"))?
            .to_le_bytes(),
    );
    for (key, value) in comments {
        let length = key.len() + 1 + value.len();
        tags.extend_from_slice(
            &u32::try_from(length)
                .map_err(|_| size_error("Ogg creation comment"))?
                .to_le_bytes(),
        );
        tags.extend_from_slice(&key);
        tags.push(b'=');
        tags.extend_from_slice(value.as_bytes());
    }

    let head = opus_head();
    let mut output = Vec::new();
    append_packet_pages(&mut output, SERIAL, 0, &head, 0x02, false)?;
    append_packet_pages(&mut output, SERIAL, 1, &tags, 0, true)?;
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "Ogg creation stream".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_ogg(&output, limits)?;
    Ok(output)
}

/// Create a new Ogg Opus metadata seed without overwriting an existing path.
pub fn create_ogg_path(
    path: impl AsRef<Path>,
    options: &OggCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing Ogg {}", path.display()),
        });
    }
    let bytes = create_ogg_to_vec(options, limits)?;
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
        validate_created_ogg(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing Ogg {}", path.display()),
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

fn opus_head() -> [u8; 19] {
    let mut head = [0_u8; 19];
    head[..8].copy_from_slice(b"OpusHead");
    head[8] = 1;
    head[9] = 2;
    head[10..12].copy_from_slice(&0_u16.to_le_bytes());
    head[12..16].copy_from_slice(&48_000_u32.to_le_bytes());
    head[16..18].copy_from_slice(&0_i16.to_le_bytes());
    head[18] = 0;
    head
}

fn append_packet_pages(
    output: &mut Vec<u8>,
    serial: u32,
    first_sequence: u32,
    packet: &[u8],
    first_flags: u8,
    eos: bool,
) -> Result<()> {
    let mut offset = 0_usize;
    let mut sequence = first_sequence;
    let mut first_page = true;
    loop {
        let remaining = packet.len().saturating_sub(offset);
        let (segment_count, final_page) = if remaining > MAX_FULL_SEGMENTS_WITH_TERMINATOR * 255 {
            (MAX_FULL_SEGMENTS_WITH_TERMINATOR, false)
        } else {
            let full_segments = remaining / 255;
            let remainder = remaining % 255;
            if remainder == 0 {
                (full_segments + usize::from(full_segments > 0), true)
            } else {
                (full_segments + 1, true)
            }
        };
        if segment_count == 0 || segment_count > MAX_SEGMENTS_PER_PAGE {
            return Err(MetraError::WriteFailure {
                message: "Ogg packet page segmentation is invalid".to_owned(),
            });
        }
        let mut lacing = Vec::with_capacity(segment_count);
        let mut body = Vec::new();
        for index in 0..segment_count {
            let segment_length = if offset >= packet.len() {
                0
            } else if index + 1 == segment_count && final_page {
                packet.len() - offset
            } else {
                255
            };
            let end = offset
                .checked_add(segment_length)
                .ok_or_else(|| size_error("Ogg packet page"))?;
            if end > packet.len() {
                return Err(MetraError::WriteFailure {
                    message: "Ogg packet page exceeds its source packet".to_owned(),
                });
            }
            lacing.push(
                u8::try_from(segment_length).map_err(|_| MetraError::WriteFailure {
                    message: "Ogg lacing value exceeds 255 bytes".to_owned(),
                })?,
            );
            body.extend_from_slice(&packet[offset..end]);
            offset = end;
        }
        let mut flags = if first_page { first_flags } else { 0x01 };
        if final_page && eos {
            flags |= 0x04;
        }
        append_page(output, serial, sequence, flags, &lacing, &body)?;
        sequence = sequence.checked_add(1).ok_or(MetraError::WriteFailure {
            message: "Ogg page sequence exceeds the 32-bit limit".to_owned(),
        })?;
        if final_page {
            return Ok(());
        }
        first_page = false;
    }
}

fn append_page(
    output: &mut Vec<u8>,
    serial: u32,
    sequence: u32,
    header_type: u8,
    lacing: &[u8],
    body: &[u8],
) -> Result<()> {
    if lacing.is_empty() || lacing.len() > MAX_SEGMENTS_PER_PAGE {
        return Err(MetraError::WriteFailure {
            message: "Ogg page must contain between one and 255 segments".to_owned(),
        });
    }
    let body_length = lacing
        .iter()
        .map(|length| usize::from(*length))
        .sum::<usize>();
    if body_length != body.len() {
        return Err(MetraError::WriteFailure {
            message: "Ogg page lacing does not match its body".to_owned(),
        });
    }
    let mut header = [0_u8; OGG_HEADER_LENGTH];
    header[..4].copy_from_slice(b"OggS");
    header[5] = header_type;
    header[14..18].copy_from_slice(&serial.to_le_bytes());
    header[18..22].copy_from_slice(&sequence.to_le_bytes());
    header[26] = u8::try_from(lacing.len()).map_err(|_| MetraError::WriteFailure {
        message: "Ogg segment count exceeds 255".to_owned(),
    })?;
    let checksum = ogg_crc(&[header.as_slice(), lacing, body].concat());
    header[22..26].copy_from_slice(&checksum.to_le_bytes());
    output.extend_from_slice(&header);
    output.extend_from_slice(lacing);
    output.extend_from_slice(body);
    Ok(())
}

fn validate_key(key: &str) -> Result<Vec<u8>> {
    if key.is_empty()
        || !key.is_ascii()
        || key.contains('=')
        || key.contains('\0')
        || key.bytes().any(|byte| byte < 0x20 || byte == 0x7F)
    {
        return Err(MetraError::InvalidTag {
            context: "Ogg creation".to_owned(),
            message: "comment keys must be printable ASCII without '='".to_owned(),
        });
    }
    Ok(key.to_ascii_uppercase().into_bytes())
}

fn validate_created_ogg(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_ogg(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.ogg".into(), bytes.len() as u64, FileFormat::Ogg),
        limits,
    )?;
    Ok(())
}

fn size_error(resource: &str) -> MetraError {
    MetraError::ResourceLimitExceeded {
        resource: resource.to_owned(),
        limit: usize::MAX,
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
        .unwrap_or("metadata.ogg");
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
    fn creates_readable_opus_head_and_tags_with_crc() {
        let options = OggCreateOptions::new()
            .with_comment("TITLE", "Metra")
            .with_comment("ARTIST", "Othmane");
        let bytes = create_ogg_to_vec(&options, ParseLimits::default()).unwrap();
        let metadata = read_ogg(
            &mut Cursor::new(bytes.clone()),
            FileInfo::new("created.ogg".into(), bytes.len() as u64, FileFormat::Ogg),
            ParseLimits::default(),
        )
        .unwrap();
        assert_eq!(metadata.find("Ogg:Codec").unwrap().display_value(), "Opus");
        assert_eq!(
            metadata.find("Ogg:SampleRate").unwrap().display_value(),
            "48000"
        );
        assert_eq!(metadata.find("Ogg:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("Ogg:Artist").unwrap().display_value(),
            "Othmane"
        );
        assert_eq!(metadata.find("Ogg:PageCount").unwrap().display_value(), "2");
    }

    #[test]
    fn rejects_duplicate_invalid_and_oversized_comments() {
        let duplicate = OggCreateOptions::new()
            .with_comment("TITLE", "one")
            .with_comment("title", "two");
        assert!(create_ogg_to_vec(&duplicate, ParseLimits::default()).is_err());

        let invalid = OggCreateOptions::new().with_comment("BAD=KEY", "value");
        assert!(create_ogg_to_vec(&invalid, ParseLimits::default()).is_err());

        let limits = ParseLimits {
            max_value_bytes: 16,
            ..ParseLimits::default()
        };
        let oversized = OggCreateOptions::new().with_comment("TITLE", "this is too large");
        assert!(create_ogg_to_vec(&oversized, limits).is_err());
    }

    #[test]
    fn path_creation_refuses_overwrite() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-ogg-create-{unique}"));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("seed.ogg");
        create_ogg_path(
            &path,
            &OggCreateOptions::new().with_comment("TITLE", "created"),
            ParseLimits::default(),
        )
        .unwrap();
        assert!(create_ogg_path(&path, &OggCreateOptions::new(), ParseLimits::default()).is_err());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}

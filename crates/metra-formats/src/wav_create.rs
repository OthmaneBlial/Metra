use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::wav::read_wav;
use crate::wav_writer::{bext_fixed_field, encode_bext_value, info_kind};

/// One bounded `LIST/INFO` value for a new WAV file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavCreateEntry {
    pub name: String,
    pub value: String,
}

impl WavCreateEntry {
    /// Create one named INFO field. The name is validated when built.
    pub fn info(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// One bounded Broadcast Wave `bext` value for a new WAV file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavBextCreateEntry {
    pub name: String,
    pub value: String,
}

impl WavBextCreateEntry {
    /// Create one named BWF field. The value is validated during creation.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// Container signature emitted by the bounded WAV creator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WavCreateKind {
    /// A classic RIFF/WAVE container with 32-bit chunk sizes.
    #[default]
    Riff,
    /// An RF64/WAVE container with a first `ds64` chunk.
    Rf64,
    /// A BW64/WAVE container with a first `ds64` chunk.
    Bw64,
}

/// Options for creating a minimal PCM WAV with bounded INFO metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WavCreateOptions {
    pub kind: WavCreateKind,
    pub info: Vec<WavCreateEntry>,
    pub bext: Vec<WavBextCreateEntry>,
}

impl WavCreateOptions {
    /// Start with no optional INFO fields.
    pub fn new() -> Self {
        Self::default()
    }

    /// Select the container signature for the generated seed.
    pub fn with_kind(mut self, kind: WavCreateKind) -> Self {
        self.kind = kind;
        self
    }

    /// Select an RF64 container for the generated seed.
    pub fn with_rf64(self) -> Self {
        self.with_kind(WavCreateKind::Rf64)
    }

    /// Select a BW64 container for the generated seed.
    pub fn with_bw64(self) -> Self {
        self.with_kind(WavCreateKind::Bw64)
    }

    /// Add one INFO field and return the updated options.
    pub fn with_info(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.info.push(WavCreateEntry::info(name, value));
        self
    }

    /// Add one INFO field in place.
    pub fn push_info(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.info.push(WavCreateEntry::info(name, value));
    }

    /// Add one Broadcast Wave `bext` field in place.
    pub fn push_bext(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.bext.push(WavBextCreateEntry::new(name, value));
    }

    /// Add one Broadcast Wave `bext` field and return the updated options.
    pub fn with_bext(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.push_bext(name, value);
        self
    }
}

/// Create a minimal 1-channel, 8-bit PCM WAV in memory.
pub fn create_wav_to_vec(options: &WavCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let mut info_entries = Vec::with_capacity(options.info.len());
    for entry in &options.info {
        let kind = info_kind(&entry.name).ok_or_else(|| MetraError::InvalidTag {
            context: "WAV creation".to_owned(),
            message: format!("unsupported INFO field {}", entry.name),
        })?;
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("WAV creation {}", entry.name),
                message: "INFO values may not contain NUL bytes".to_owned(),
            });
        }
        if entry.value.len() >= limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("WAV creation value {}", entry.name),
                limit: limits.max_value_bytes,
            });
        }
        if info_entries
            .iter()
            .any(|existing: &([u8; 4], String, String)| existing.0 == kind)
        {
            return Err(MetraError::InvalidTag {
                context: "WAV creation".to_owned(),
                message: format!("duplicate INFO field {}", entry.name),
            });
        }
        info_entries.push((kind, entry.name.clone(), entry.value.clone()));
    }

    let mut bext = None;
    if !options.bext.is_empty() {
        let mut data = vec![0_u8; 602];
        for entry in &options.bext {
            if options
                .bext
                .iter()
                .filter(|existing| existing.name == entry.name)
                .count()
                != 1
            {
                return Err(MetraError::InvalidTag {
                    context: "WAV BWF creation".to_owned(),
                    message: format!("duplicate bext field {}", entry.name),
                });
            }
            let encoded = encode_bext_value(&entry.name, &entry.value, limits)?;
            if entry.name == "CodingHistory" {
                data.extend_from_slice(&encoded);
            } else {
                let (start, width) =
                    bext_fixed_field(&entry.name).ok_or_else(|| MetraError::InvalidTag {
                        context: "WAV BWF creation".to_owned(),
                        message: format!("unsupported bext field {}", entry.name),
                    })?;
                if encoded.len() != width {
                    return Err(MetraError::InvalidTag {
                        context: format!("WAV BWF creation {}", entry.name),
                        message: "encoded value has the wrong fixed width".to_owned(),
                    });
                }
                data[start..start + width].copy_from_slice(&encoded);
            }
        }
        if data.len() > limits.max_metadata_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: "WAV BWF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            });
        }
        bext = Some(data);
    }

    let mut info = b"INFO".to_vec();
    for (kind, _, value) in &info_entries {
        let length =
            value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "WAV creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        let length = u32::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "WAV creation value".to_owned(),
            limit: limits.max_value_bytes,
        })?;
        info.extend_from_slice(kind);
        info.extend_from_slice(&length.to_le_bytes());
        info.extend_from_slice(value.as_bytes());
        info.push(0);
        if length % 2 == 1 {
            info.push(0);
        }
    }

    let mut fmt = Vec::with_capacity(16);
    fmt.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1_u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&8_000_u32.to_le_bytes());
    fmt.extend_from_slice(&8_000_u32.to_le_bytes());
    fmt.extend_from_slice(&1_u16.to_le_bytes()); // block align
    fmt.extend_from_slice(&8_u16.to_le_bytes()); // bits per sample
    let mut body = Vec::new();
    if !matches!(options.kind, WavCreateKind::Riff) {
        // The generated seed has one byte of PCM data and one sample. RF64 and
        // BW64 keep the data chunk at the 32-bit sentinel while ds64 carries
        // its real 64-bit size.
        let mut ds64 = vec![0_u8; 28];
        ds64[8..16].copy_from_slice(&1_u64.to_le_bytes());
        ds64[16..24].copy_from_slice(&1_u64.to_le_bytes());
        write_chunk(&mut body, b"ds64", &ds64)?;
    }
    write_chunk(&mut body, b"fmt ", &fmt)?;
    if let Some(bext) = bext {
        write_chunk(&mut body, b"bext", &bext)?;
    }
    write_chunk(&mut body, b"LIST", &info)?;
    if matches!(options.kind, WavCreateKind::Riff) {
        write_chunk(&mut body, b"data", &[0])?;
    } else {
        body.extend_from_slice(b"data");
        body.extend_from_slice(&u32::MAX.to_le_bytes());
        body.push(0);
        body.push(0);
    }

    let riff_size =
        4_usize
            .checked_add(body.len())
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "WAV creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    if !matches!(options.kind, WavCreateKind::Riff) {
        let riff_size =
            u64::try_from(riff_size).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "WAV RF64 creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
        body[8..16].copy_from_slice(&riff_size.to_le_bytes());
    }
    let mut output = match options.kind {
        WavCreateKind::Riff => b"RIFF".to_vec(),
        WavCreateKind::Rf64 => b"RF64".to_vec(),
        WavCreateKind::Bw64 => b"BW64".to_vec(),
    };
    if matches!(options.kind, WavCreateKind::Riff) {
        let riff_size =
            u32::try_from(riff_size).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "WAV creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
        output.extend_from_slice(&riff_size.to_le_bytes());
    } else {
        output.extend_from_slice(&u32::MAX.to_le_bytes());
    }
    output.extend_from_slice(b"WAVE");
    output.extend_from_slice(&body);
    if output.len() > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "WAV creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    validate_created_wav(&output, limits)?;
    Ok(output)
}

/// Create a new WAV path without overwriting an existing destination.
pub fn create_wav_path(
    path: impl AsRef<Path>,
    options: &WavCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing WAV {}", path.display()),
        });
    }
    let bytes = create_wav_to_vec(options, limits)?;
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
        validate_created_wav(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing WAV {}", path.display()),
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

fn validate_created_wav(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_wav(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.wav".into(), bytes.len() as u64, FileFormat::Wav),
        limits,
    )?;
    Ok(())
}

fn write_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len()).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: "WAV creation chunk".to_owned(),
        limit: u32::MAX as usize,
    })?;
    output.extend_from_slice(kind);
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(data);
    if data.len() % 2 == 1 {
        output.push(0);
    }
    Ok(())
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.wav");
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

fn write_error(path: &Path, source: std::io::Error) -> MetraError {
    MetraError::WriteFailure {
        message: format!("{}: {source}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn creates_readable_pcm_wav_with_info_metadata() {
        let options = WavCreateOptions::new()
            .with_info("Title", "Metra")
            .with_info("Artist", "Othmane");
        let bytes = create_wav_to_vec(&options, ParseLimits::default())
            .expect("minimal WAV creation should succeed");
        let metadata = read_wav(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.wav".into(), bytes.len() as u64, FileFormat::Wav),
            ParseLimits::default(),
        )
        .expect("created WAV should remain readable");
        assert_eq!(metadata.find("WAV:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("WAV:Artist").unwrap().display_value(),
            "Othmane"
        );
        assert_eq!(metadata.find("WAV:Channels").unwrap().display_value(), "1");
    }

    #[test]
    fn creates_readable_broadcast_wave_seed() {
        let options = WavCreateOptions::new()
            .with_bext("Description", "Metra take")
            .with_bext("DateTimeOriginal", "2026:09:14 12:34:56")
            .with_bext("TimeReference", "17")
            .with_bext("BWFVersion", "2")
            .with_bext("BWF_UMID", "AB".repeat(32))
            .with_bext("CodingHistory", "A=PCM,F=48000,W=8,M=mono");
        let bytes = create_wav_to_vec(&options, ParseLimits::default())
            .expect("BWF WAV creation should succeed");
        let metadata = read_wav(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new(
                "created-bwf.wav".into(),
                bytes.len() as u64,
                FileFormat::Wav,
            ),
            ParseLimits::default(),
        )
        .expect("created BWF should remain readable");
        assert_eq!(
            metadata.find("WAV:Description").unwrap().display_value(),
            "Metra take"
        );
        assert_eq!(
            metadata
                .find("WAV:DateTimeOriginal")
                .unwrap()
                .display_value(),
            "2026:09:14 12:34:56"
        );
        assert_eq!(
            metadata.find("WAV:BWF_UMID").unwrap().display_value(),
            "AB".repeat(32)
        );
        assert_eq!(
            metadata.find("WAV:CodingHistory").unwrap().display_value(),
            "A=PCM,F=48000,W=8,M=mono"
        );
    }

    #[test]
    fn creates_readable_rf64_and_bw64_seeds() {
        for kind in [WavCreateKind::Rf64, WavCreateKind::Bw64] {
            let options = WavCreateOptions::new()
                .with_kind(kind)
                .with_info("Title", "Metra")
                .with_bext("Description", "RF64 seed");
            let bytes = create_wav_to_vec(&options, ParseLimits::default())
                .expect("RF64/BW64 creation should succeed");
            let signature = match kind {
                WavCreateKind::Rf64 => b"RF64",
                WavCreateKind::Bw64 => b"BW64",
                WavCreateKind::Riff => unreachable!("the loop only covers extended WAV kinds"),
            };
            assert_eq!(&bytes[..4], signature);
            assert_eq!(
                u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
                u32::MAX
            );
            assert_eq!(&bytes[12..16], b"ds64");
            assert_eq!(
                u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
                (bytes.len() - 8) as u64
            );
            let metadata = read_wav(
                &mut std::io::Cursor::new(bytes.clone()),
                FileInfo::new(
                    "created-rf64.wav".into(),
                    bytes.len() as u64,
                    FileFormat::Wav,
                ),
                ParseLimits::default(),
            )
            .expect("RF64/BW64 seed should remain readable");
            assert_eq!(metadata.find("WAV:Title").unwrap().display_value(), "Metra");
            assert_eq!(
                metadata.find("WAV:DataSize64").unwrap().display_value(),
                "1"
            );
            assert_eq!(
                metadata
                    .find("WAV:NumberOfSamples64")
                    .unwrap()
                    .display_value(),
                "1"
            );
            assert!(bytes.windows(10).any(|window| {
                window[..8] == [b'd', b'a', b't', b'a', 0xff, 0xff, 0xff, 0xff]
                    && window[8..] == [0, 0]
            }));
        }
    }

    #[test]
    fn rejects_invalid_duplicate_and_oversized_info() {
        let invalid = WavCreateOptions::new().with_info("Unknown", "value");
        assert!(matches!(
            create_wav_to_vec(&invalid, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let duplicate = WavCreateOptions::new()
            .with_info("Title", "one")
            .with_info("Title", "two");
        assert!(matches!(
            create_wav_to_vec(&duplicate, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        let oversized = WavCreateOptions::new().with_info("Title", "value");
        assert!(matches!(
            create_wav_to_vec(&oversized, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));

        let duplicate_bext = WavCreateOptions::new()
            .with_bext("Description", "one")
            .with_bext("Description", "two");
        assert!(matches!(
            create_wav_to_vec(&duplicate_bext, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let invalid_bext = WavCreateOptions::new().with_bext("TimeReference", "not-a-number");
        assert!(create_wav_to_vec(&invalid_bext, ParseLimits::default()).is_err());
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-wav-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.wav");
        let options = WavCreateOptions::new().with_info("Software", "Metra");

        create_wav_path(&path, &options, ParseLimits::default())
            .expect("new WAV path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_wav_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created WAV should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}

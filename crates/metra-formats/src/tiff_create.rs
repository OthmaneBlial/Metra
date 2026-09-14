use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use metra_core::{FileFormat, FileInfo, MetraError, ParseLimits, Result};

use crate::atomic::atomic_replace;
use crate::tiff::read_tiff;

const BASE_ENTRY_COUNT: usize = 9;

/// One bounded ASCII value to place in the initial IFD of a new TIFF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiffCreateEntry {
    pub key: String,
    pub value: String,
}

impl TiffCreateEntry {
    /// Create a canonical EXIF ASCII entry such as `Make` or `Artist`.
    pub fn ascii(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// Bounded GPS values to place in a new TIFF GPS IFD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiffGpsCreateOptions {
    pub latitude: String,
    pub longitude: String,
    pub altitude_meters: Option<String>,
    pub image_direction_degrees: Option<String>,
    pub speed_meters_per_second: Option<String>,
    pub time_of_day_seconds: Option<String>,
    pub date: Option<String>,
}

impl TiffGpsCreateOptions {
    /// Create a GPS seed from decimal latitude and longitude values.
    pub fn new(latitude: impl Into<String>, longitude: impl Into<String>) -> Self {
        Self {
            latitude: latitude.into(),
            longitude: longitude.into(),
            altitude_meters: None,
            image_direction_degrees: None,
            speed_meters_per_second: None,
            time_of_day_seconds: None,
            date: None,
        }
    }

    /// Add signed altitude in meters to the GPS seed.
    pub fn with_altitude_meters(mut self, value: impl Into<String>) -> Self {
        self.altitude_meters = Some(value.into());
        self
    }

    /// Add image direction in degrees, bounded to 0..=360.
    pub fn with_image_direction_degrees(mut self, value: impl Into<String>) -> Self {
        self.image_direction_degrees = Some(value.into());
        self
    }

    /// Add speed in meters per second; the seed stores it in kilometers/hour.
    pub fn with_speed_meters_per_second(mut self, value: impl Into<String>) -> Self {
        self.speed_meters_per_second = Some(value.into());
        self
    }

    /// Add seconds since midnight to the GPS timestamp.
    pub fn with_time_of_day_seconds(mut self, value: impl Into<String>) -> Self {
        self.time_of_day_seconds = Some(value.into());
        self
    }

    /// Add a validated `YYYY:MM:DD` GPS date.
    pub fn with_date(mut self, value: impl Into<String>) -> Self {
        self.date = Some(value.into());
        self
    }
}

/// Options for creating a minimal, readable classic TIFF container.
///
/// The generated file contains a valid 1x1 monochrome image and an IFD0 with
/// the supplied ASCII EXIF fields. It is intended as a safe metadata seed,
/// not as a general image encoder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TiffCreateOptions {
    pub entries: Vec<TiffCreateEntry>,
    pub gps: Option<TiffGpsCreateOptions>,
}

impl TiffCreateOptions {
    /// Start with no optional metadata entries.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one ASCII EXIF entry and return the updated options.
    pub fn with_ascii(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push(TiffCreateEntry::ascii(key, value));
        self
    }

    /// Add one ASCII EXIF entry in place.
    pub fn push_ascii(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.push(TiffCreateEntry::ascii(key, value));
    }

    /// Add a bounded GPS latitude/longitude pair to the new TIFF.
    pub fn with_gps_coordinates(
        mut self,
        latitude: impl Into<String>,
        longitude: impl Into<String>,
    ) -> Self {
        let gps = self
            .gps
            .get_or_insert_with(|| TiffGpsCreateOptions::new("", ""));
        gps.latitude = latitude.into();
        gps.longitude = longitude.into();
        self
    }

    /// Add signed altitude in meters to the GPS seed.
    pub fn with_gps_altitude_meters(mut self, value: impl Into<String>) -> Self {
        self.gps_options_mut().altitude_meters = Some(value.into());
        self
    }

    /// Add image direction in degrees to the GPS seed.
    pub fn with_gps_image_direction_degrees(mut self, value: impl Into<String>) -> Self {
        self.gps_options_mut().image_direction_degrees = Some(value.into());
        self
    }

    /// Add speed in meters per second to the GPS seed.
    pub fn with_gps_speed_meters_per_second(mut self, value: impl Into<String>) -> Self {
        self.gps_options_mut().speed_meters_per_second = Some(value.into());
        self
    }

    /// Add seconds since midnight to the GPS seed.
    pub fn with_gps_time_of_day_seconds(mut self, value: impl Into<String>) -> Self {
        self.gps_options_mut().time_of_day_seconds = Some(value.into());
        self
    }

    /// Add a `YYYY:MM:DD` date to the GPS seed.
    pub fn with_gps_date(mut self, value: impl Into<String>) -> Self {
        self.gps_options_mut().date = Some(value.into());
        self
    }

    fn gps_options_mut(&mut self) -> &mut TiffGpsCreateOptions {
        self.gps
            .get_or_insert_with(|| TiffGpsCreateOptions::new("", ""))
    }
}

/// Create a minimal classic little-endian TIFF in memory.
pub fn create_tiff_to_vec(options: &TiffCreateOptions, limits: ParseLimits) -> Result<Vec<u8>> {
    let gps = options
        .gps
        .as_ref()
        .map(|gps| encode_gps_coordinates(gps, limits))
        .transpose()?;
    let mut entries = Vec::with_capacity(options.entries.len());
    for entry in &options.entries {
        let tag = ascii_tag_id(&entry.key).ok_or_else(|| MetraError::InvalidTag {
            context: "TIFF creation".to_owned(),
            message: format!("unsupported ASCII creation key {}", entry.key),
        })?;
        if entry.value.as_bytes().contains(&0) {
            return Err(MetraError::InvalidTag {
                context: format!("TIFF creation {}", entry.key),
                message: "ASCII values may not contain NUL bytes".to_owned(),
            });
        }
        let value_len =
            entry
                .value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "TIFF creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        if value_len > limits.max_value_bytes {
            return Err(MetraError::ResourceLimitExceeded {
                resource: format!("TIFF creation value {}", entry.key),
                limit: limits.max_value_bytes,
            });
        }
        if entries.iter().any(|(existing, _, _)| *existing == tag) {
            return Err(MetraError::InvalidTag {
                context: "TIFF creation".to_owned(),
                message: format!("duplicate ASCII creation key {}", entry.key),
            });
        }
        entries.push((tag, entry.key.clone(), entry.value.clone()));
    }

    let entry_count = BASE_ENTRY_COUNT
        .checked_add(entries.len())
        .and_then(|count| count.checked_add(usize::from(gps.is_some())))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "TIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        })?;
    if entry_count > limits.max_ifd_entries || entry_count > u16::MAX as usize {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    entries.sort_by_key(|(tag, _, _)| *tag);

    let ifd_bytes = entry_count
        .checked_mul(12)
        .and_then(|bytes| bytes.checked_add(6))
        .ok_or_else(|| MetraError::ResourceLimitExceeded {
            resource: "TIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    let ifd_end =
        8usize
            .checked_add(ifd_bytes)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let gps_ifd_bytes = gps
        .as_ref()
        .map(|gps| gps.ifd_bytes(4, 12, 2, 4))
        .unwrap_or(0);
    let data_offset =
        ifd_end
            .checked_add(gps_ifd_bytes)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let ascii_bytes = entries.iter().try_fold(0usize, |total, (_, _, value)| {
        total
            .checked_add(value.len() + 1)
            .ok_or(MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })
    })?;
    let pixel_offset =
        data_offset
            .checked_add(ascii_bytes)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    let output_len =
        pixel_offset
            .checked_add(1)
            .ok_or_else(|| MetraError::ResourceLimitExceeded {
                resource: "TIFF creation metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    if output_len > limits.max_metadata_bytes || pixel_offset > u32::MAX as usize {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF creation metadata".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }

    let mut output = Vec::with_capacity(output_len);
    output.extend_from_slice(b"II");
    output.extend_from_slice(&42u16.to_le_bytes());
    output.extend_from_slice(&8u32.to_le_bytes());
    output.extend_from_slice(&(entry_count as u16).to_le_bytes());

    push_long_entry(&mut output, 0x0100, 1); // ImageWidth
    push_long_entry(&mut output, 0x0101, 1); // ImageLength
    push_short_entry(&mut output, 0x0102, 8); // BitsPerSample
    push_short_entry(&mut output, 0x0103, 1); // Compression: none
    push_short_entry(&mut output, 0x0106, 1); // PhotometricInterpretation
    push_long_entry(&mut output, 0x0111, pixel_offset as u32); // StripOffsets
    push_short_entry(&mut output, 0x0115, 1); // SamplesPerPixel
    push_long_entry(&mut output, 0x0116, 1); // RowsPerStrip
    push_long_entry(&mut output, 0x0117, 1); // StripByteCounts
    if gps.is_some() {
        push_long_entry(&mut output, 0x8825, ifd_end as u32); // GPSInfoIFDPointer
    }

    let mut next_data_offset = data_offset;
    for (tag, _, value) in &entries {
        let length =
            value
                .len()
                .checked_add(1)
                .ok_or_else(|| MetraError::ResourceLimitExceeded {
                    resource: "TIFF creation value".to_owned(),
                    limit: limits.max_value_bytes,
                })?;
        output.extend_from_slice(&tag.to_le_bytes());
        output.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        output.extend_from_slice(&(length as u32).to_le_bytes());
        output.extend_from_slice(&(next_data_offset as u32).to_le_bytes());
        next_data_offset += length;
    }
    output.extend_from_slice(&0u32.to_le_bytes());

    if let Some(gps) = &gps {
        write_gps_ifd(&mut output, ifd_end, gps);
    }

    for (_, _, value) in &entries {
        output.extend_from_slice(value.as_bytes());
        output.push(0);
    }
    output.push(0);

    debug_assert_eq!(output.len(), output_len);
    validate_created_tiff(&output, limits)?;
    Ok(output)
}

/// Create a new TIFF path without overwriting an existing destination.
pub fn create_tiff_path(
    path: impl AsRef<Path>,
    options: &TiffCreateOptions,
    limits: ParseLimits,
) -> Result<()> {
    let path = path.as_ref().to_path_buf();
    if path.exists() {
        return Err(MetraError::WriteFailure {
            message: format!("refusing to overwrite existing TIFF {}", path.display()),
        });
    }
    let bytes = create_tiff_to_vec(options, limits)?;
    let temp_path = temporary_path(&path)?;
    let result = (|| {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|source| MetraError::WriteFailure {
                message: format!("cannot create {}: {source}", temp_path.display()),
            })?;
        output
            .write_all(&bytes)
            .map_err(|source| write_error(&temp_path, source))?;
        output
            .sync_all()
            .map_err(|source| write_error(&temp_path, source))?;
        drop(output);
        validate_created_tiff(&bytes, limits)?;
        if path.exists() {
            return Err(MetraError::WriteFailure {
                message: format!("refusing to overwrite existing TIFF {}", path.display()),
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

fn validate_created_tiff(bytes: &[u8], limits: ParseLimits) -> Result<()> {
    read_tiff(
        &mut std::io::Cursor::new(bytes),
        FileInfo::new("created.tif".into(), bytes.len() as u64, FileFormat::Tiff),
        limits,
    )?;
    Ok(())
}

pub(crate) fn ascii_tag_id(key: &str) -> Option<u16> {
    let key = key
        .strip_prefix("TIFF:")
        .or_else(|| key.strip_prefix("EXIF:"))
        .unwrap_or(key);
    match key {
        "ImageDescription" => Some(0x010E),
        "Make" => Some(0x010F),
        "Model" => Some(0x0110),
        "Software" => Some(0x0131),
        "Artist" => Some(0x013B),
        "Copyright" => Some(0x8298),
        _ => None,
    }
}

fn push_short_entry(output: &mut Vec<u8>, tag: u16, value: u16) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&3u16.to_le_bytes());
    output.extend_from_slice(&1u32.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
    output.extend_from_slice(&0u16.to_le_bytes());
}

fn push_long_entry(output: &mut Vec<u8>, tag: u16, value: u32) {
    output.extend_from_slice(&tag.to_le_bytes());
    output.extend_from_slice(&4u16.to_le_bytes());
    output.extend_from_slice(&1u32.to_le_bytes());
    output.extend_from_slice(&value.to_le_bytes());
}

#[derive(Debug)]
pub(crate) struct EncodedGpsCoordinates {
    pub(crate) entries: Vec<EncodedGpsEntry>,
}

#[derive(Debug)]
pub(crate) struct EncodedGpsEntry {
    pub(crate) tag: u16,
    pub(crate) type_id: u16,
    pub(crate) count: u32,
    pub(crate) bytes: Vec<u8>,
}

impl EncodedGpsCoordinates {
    pub(crate) fn ifd_bytes(
        &self,
        inline_value_size: usize,
        entry_size: usize,
        count_size: usize,
        next_offset_size: usize,
    ) -> usize {
        let payload_bytes = self
            .entries
            .iter()
            .filter(|entry| entry.bytes.len() > inline_value_size)
            .map(|entry| entry.bytes.len())
            .sum::<usize>();
        count_size + self.entries.len() * entry_size + next_offset_size + payload_bytes
    }
}

pub(crate) fn encode_gps_coordinates(
    options: &TiffGpsCreateOptions,
    limits: ParseLimits,
) -> Result<EncodedGpsCoordinates> {
    let latitude = parse_coordinate(&options.latitude, 90.0, b'N', b'S', "latitude")?;
    let longitude = parse_coordinate(&options.longitude, 180.0, b'E', b'W', "longitude")?;
    let mut entries = vec![
        gps_bytes_entry(0x0000, 1, 4, vec![2, 3, 0, 0]),
        gps_bytes_entry(0x0001, 2, 2, vec![latitude.reference, 0]),
        gps_rational_entry(0x0002, &latitude.parts),
        gps_bytes_entry(0x0003, 2, 2, vec![longitude.reference, 0]),
        gps_rational_entry(0x0004, &longitude.parts),
    ];
    if let Some(value) = &options.altitude_meters {
        let altitude = parse_decimal(value, "altitude")?;
        let negative = altitude.is_sign_negative() && altitude != 0.0;
        entries.push(gps_bytes_entry(0x0005, 1, 1, vec![u8::from(negative)]));
        entries.push(gps_rational_entry(
            0x0006,
            &[bounded_rational(altitude.abs(), "altitude")?],
        ));
    }
    if let Some(value) = &options.time_of_day_seconds {
        entries.push(gps_rational_entry(
            0x0007,
            &encode_gps_time(parse_decimal(value, "time")?, "time")?,
        ));
    }
    if let Some(value) = &options.speed_meters_per_second {
        let speed = parse_decimal(value, "speed")?;
        if speed < 0.0 {
            return Err(invalid_gps("speed must be non-negative"));
        }
        let kilometers_per_hour = speed * 3.6;
        if !kilometers_per_hour.is_finite() {
            return Err(invalid_gps("speed cannot be represented safely"));
        }
        entries.push(gps_bytes_entry(0x000C, 2, 2, vec![b'K', 0]));
        entries.push(gps_rational_entry(
            0x000D,
            &[bounded_rational(kilometers_per_hour, "speed")?],
        ));
    }
    if let Some(value) = &options.image_direction_degrees {
        let direction = parse_decimal(value, "image direction")?;
        if !(0.0..=360.0).contains(&direction) {
            return Err(invalid_gps("image direction must be within 0..=360"));
        }
        entries.push(gps_rational_entry(
            0x0011,
            &[bounded_rational(direction, "image direction")?],
        ));
    }
    if let Some(value) = &options.date {
        validate_gps_date(value)?;
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        entries.push(gps_bytes_entry(0x001D, 2, 11, bytes));
    }
    entries.sort_by_key(|entry| entry.tag);
    if entries.len() > limits.max_ifd_entries {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF GPS creation IFD entries".to_owned(),
            limit: limits.max_ifd_entries,
        });
    }
    let largest_value = entries
        .iter()
        .map(|entry| entry.bytes.len())
        .max()
        .unwrap_or(0);
    if largest_value > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "TIFF GPS creation values".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    Ok(EncodedGpsCoordinates { entries })
}

fn gps_bytes_entry(tag: u16, type_id: u16, count: u32, bytes: Vec<u8>) -> EncodedGpsEntry {
    EncodedGpsEntry {
        tag,
        type_id,
        count,
        bytes,
    }
}

fn gps_rational_entry(tag: u16, values: &[(u32, u32)]) -> EncodedGpsEntry {
    let mut bytes = Vec::with_capacity(values.len() * 8);
    for (numerator, denominator) in values {
        bytes.extend_from_slice(&numerator.to_le_bytes());
        bytes.extend_from_slice(&denominator.to_le_bytes());
    }
    gps_bytes_entry(tag, 5, values.len() as u32, bytes)
}

#[derive(Debug)]
struct EncodedCoordinate {
    reference: u8,
    parts: [(u32, u32); 3],
}

fn parse_coordinate(
    raw: &str,
    maximum: f64,
    positive_reference: u8,
    negative_reference: u8,
    label: &str,
) -> Result<EncodedCoordinate> {
    let value = parse_decimal(raw, label)?;
    if value.abs() > maximum {
        return Err(MetraError::InvalidTag {
            context: "TIFF GPS creation".to_owned(),
            message: format!("{label} must be finite and within -{maximum}..={maximum}"),
        });
    }
    let absolute = value.abs();
    let degrees = absolute.floor();
    let minute_value = (absolute - degrees) * 60.0;
    let mut minutes = minute_value.floor();
    let mut seconds = (minute_value - minutes) * 60.0;
    if seconds >= 60.0 {
        seconds = 0.0;
        minutes += 1.0;
    }
    let mut degrees = degrees;
    if minutes >= 60.0 {
        minutes = 0.0;
        degrees += 1.0;
    }
    if degrees > maximum {
        return Err(MetraError::InvalidTag {
            context: "TIFF GPS creation".to_owned(),
            message: format!("{label} rounds outside the supported range"),
        });
    }
    Ok(EncodedCoordinate {
        reference: if value.is_sign_negative() && value != 0.0 {
            negative_reference
        } else {
            positive_reference
        },
        parts: [
            bounded_rational(degrees, label)?,
            bounded_rational(minutes, label)?,
            bounded_rational(seconds, label)?,
        ],
    })
}

fn parse_decimal(raw: &str, label: &str) -> Result<f64> {
    let value = raw
        .parse::<f64>()
        .map_err(|_| invalid_gps(format!("{label} {raw:?} is not a decimal number")))?;
    if !value.is_finite() {
        return Err(invalid_gps(format!("{label} must be finite")));
    }
    Ok(value)
}

fn invalid_gps(message: impl Into<String>) -> MetraError {
    MetraError::InvalidTag {
        context: "TIFF GPS creation".to_owned(),
        message: message.into(),
    }
}

fn bounded_rational(value: f64, label: &str) -> Result<(u32, u32)> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u32::MAX) {
        return Err(MetraError::InvalidTag {
            context: "TIFF GPS creation".to_owned(),
            message: format!("{label} component is outside the rational range"),
        });
    }
    if value == 0.0 {
        return Ok((0, 1));
    }
    let denominator_limit = (f64::from(u32::MAX) / value).floor().min(1_000_000.0);
    let denominator = denominator_limit.max(1.0) as u32;
    let numerator = (value * f64::from(denominator)).round();
    if !numerator.is_finite() || numerator > f64::from(u32::MAX) {
        return Err(MetraError::InvalidTag {
            context: "TIFF GPS creation".to_owned(),
            message: format!("{label} component cannot be represented safely"),
        });
    }
    Ok((numerator as u32, denominator))
}

fn encode_gps_time(value: f64, label: &str) -> Result<[(u32, u32); 3]> {
    const MICROS_PER_SECOND: u64 = 1_000_000;
    const MICROS_PER_DAY: u64 = 86_400 * MICROS_PER_SECOND;
    if !value.is_finite() || !(0.0..86_400.0).contains(&value) {
        return Err(invalid_gps(format!(
            "{label} must be finite and within 0..86400 seconds"
        )));
    }
    let micros = (value * MICROS_PER_SECOND as f64).round();
    if !micros.is_finite() || micros < 0.0 || micros >= MICROS_PER_DAY as f64 {
        return Err(invalid_gps(format!("{label} rounds outside the day")));
    }
    let micros = micros as u64;
    let hours = micros / (3_600 * MICROS_PER_SECOND);
    let remainder = micros % (3_600 * MICROS_PER_SECOND);
    let minutes = remainder / (60 * MICROS_PER_SECOND);
    let seconds_micros = remainder % (60 * MICROS_PER_SECOND);
    Ok([
        (hours as u32, 1),
        (minutes as u32, 1),
        (seconds_micros as u32, MICROS_PER_SECOND as u32),
    ])
}

fn validate_gps_date(value: &str) -> Result<()> {
    let mut parts = value.split(':');
    let year = parts
        .next()
        .filter(|part| part.len() == 4)
        .and_then(|part| part.parse::<u16>().ok());
    let month = parts
        .next()
        .filter(|part| part.len() == 2)
        .and_then(|part| part.parse::<u8>().ok());
    let day = parts
        .next()
        .filter(|part| part.len() == 2)
        .and_then(|part| part.parse::<u8>().ok());
    let valid = parts.next().is_none()
        && year.is_some()
        && month.is_some_and(|month| (1..=12).contains(&month))
        && day.is_some_and(|day| {
            let year = year.expect("year was checked above");
            let month = month.expect("month was checked above");
            let leap =
                year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
            let days = match month {
                2 if leap => 29,
                2 => 28,
                4 | 6 | 9 | 11 => 30,
                _ => 31,
            };
            (1..=days).contains(&day)
        });
    if valid {
        Ok(())
    } else {
        Err(invalid_gps(format!(
            "date {value:?} must use a valid YYYY:MM:DD value"
        )))
    }
}

fn write_gps_ifd(output: &mut Vec<u8>, ifd_offset: usize, gps: &EncodedGpsCoordinates) {
    debug_assert_eq!(output.len(), ifd_offset);
    let values_offset = ifd_offset + 2 + gps.entries.len() * 12 + 4;
    output.extend_from_slice(&(gps.entries.len() as u16).to_le_bytes());
    let mut payload_offset = values_offset;
    for entry in &gps.entries {
        output.extend_from_slice(&entry.tag.to_le_bytes());
        output.extend_from_slice(&entry.type_id.to_le_bytes());
        output.extend_from_slice(&entry.count.to_le_bytes());
        if entry.bytes.len() <= 4 {
            output.extend_from_slice(&entry.bytes);
            output.resize(output.len() + (4 - entry.bytes.len()), 0);
        } else {
            output.extend_from_slice(&(payload_offset as u32).to_le_bytes());
            payload_offset += entry.bytes.len();
        }
    }
    output.extend_from_slice(&0_u32.to_le_bytes());
    for entry in &gps.entries {
        if entry.bytes.len() > 4 {
            output.extend_from_slice(&entry.bytes);
        }
    }
    debug_assert_eq!(output.len(), ifd_offset + gps.ifd_bytes(4, 12, 2, 4));
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("created.tif");
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
    use metra_core::TagValue;

    #[test]
    fn creates_readable_one_pixel_tiff_with_ascii_metadata() {
        let options = TiffCreateOptions::new()
            .with_ascii("EXIF:Make", "Metra")
            .with_ascii("Artist", "Othmane");
        let bytes = create_tiff_to_vec(&options, ParseLimits::default())
            .expect("minimal TIFF creation should succeed");
        let metadata = read_tiff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new("created.tif".into(), bytes.len() as u64, FileFormat::Tiff),
            ParseLimits::default(),
        )
        .expect("created TIFF should remain readable");
        assert_eq!(
            metadata.find("EXIF:Make").unwrap().value,
            TagValue::String("Metra".into())
        );
        assert_eq!(
            metadata.find("EXIF:Artist").unwrap().display_value(),
            "Othmane"
        );
        assert_eq!(
            metadata.find("EXIF:ImageWidth").unwrap().display_value(),
            "1"
        );
    }

    #[test]
    fn creates_readable_tiff_with_gps_coordinates() {
        let options = TiffCreateOptions::new().with_gps_coordinates("-48.8566", "2.3522");
        let bytes = create_tiff_to_vec(&options, ParseLimits::default())
            .expect("GPS TIFF creation should succeed");
        let metadata = read_tiff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new(
                "created-gps.tif".into(),
                bytes.len() as u64,
                FileFormat::Tiff,
            ),
            ParseLimits::default(),
        )
        .expect("created GPS TIFF should remain readable");

        let latitude = metadata
            .find("GPS:LatitudeDecimal")
            .expect("derived latitude should be present")
            .display_value()
            .parse::<f64>()
            .expect("derived latitude should be numeric");
        let longitude = metadata
            .find("GPS:LongitudeDecimal")
            .expect("derived longitude should be present")
            .display_value()
            .parse::<f64>()
            .expect("derived longitude should be numeric");
        assert!((latitude + 48.8566).abs() < 0.000001);
        assert!((longitude - 2.3522).abs() < 0.000001);
        assert_eq!(
            metadata.find("GPS:GPSLatitudeRef").unwrap().display_value(),
            "S"
        );
        assert_eq!(
            metadata
                .find("GPS:GPSLongitudeRef")
                .unwrap()
                .display_value(),
            "E"
        );
    }

    #[test]
    fn creates_readable_tiff_with_optional_gps_values() {
        let options = TiffCreateOptions::new()
            .with_gps_coordinates("48.8566", "2.3522")
            .with_gps_altitude_meters("-125.5")
            .with_gps_image_direction_degrees("271.25")
            .with_gps_speed_meters_per_second("10")
            .with_gps_time_of_day_seconds("45296.125")
            .with_gps_date("2026:09:14");
        let bytes = create_tiff_to_vec(&options, ParseLimits::default())
            .expect("complete GPS TIFF creation should succeed");
        let metadata = read_tiff(
            &mut std::io::Cursor::new(bytes.clone()),
            FileInfo::new(
                "created-gps-full.tif".into(),
                bytes.len() as u64,
                FileFormat::Tiff,
            ),
            ParseLimits::default(),
        )
        .expect("created complete GPS TIFF should remain readable");

        for (key, expected) in [
            ("GPS:AltitudeMeters", -125.5),
            ("GPS:ImageDirectionDegrees", 271.25),
            ("GPS:SpeedMetersPerSecond", 10.0),
            ("GPS:TimeOfDaySeconds", 45296.125),
        ] {
            let actual = metadata
                .find(key)
                .unwrap_or_else(|| panic!("{key} should be present"))
                .display_value()
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("{key} should be numeric"));
            assert!(
                (actual - expected).abs() < 0.000001,
                "{key}: {actual} != {expected}"
            );
        }
        assert_eq!(
            metadata.find("GPS:GPSAltitudeRef").unwrap().display_value(),
            "1"
        );
        assert_eq!(
            metadata.find("GPS:GPSSpeedRef").unwrap().display_value(),
            "K"
        );
        assert_eq!(
            metadata.find("GPS:GPSDateStamp").unwrap().display_value(),
            "2026-09-14"
        );
    }

    #[test]
    fn rejects_out_of_range_gps_coordinates() {
        for (latitude, longitude) in [("90.0001", "0"), ("NaN", "0"), ("0", "180.0001")] {
            let options = TiffCreateOptions::new().with_gps_coordinates(latitude, longitude);
            assert!(matches!(
                create_tiff_to_vec(&options, ParseLimits::default()),
                Err(MetraError::InvalidTag { .. })
            ));
        }
    }

    #[test]
    fn rejects_invalid_optional_gps_values() {
        let invalid_options = [
            TiffCreateOptions::new()
                .with_gps_coordinates("0", "0")
                .with_gps_altitude_meters("NaN"),
            TiffCreateOptions::new()
                .with_gps_coordinates("0", "0")
                .with_gps_image_direction_degrees("360.1"),
            TiffCreateOptions::new()
                .with_gps_coordinates("0", "0")
                .with_gps_speed_meters_per_second("-1"),
            TiffCreateOptions::new()
                .with_gps_coordinates("0", "0")
                .with_gps_time_of_day_seconds("86400"),
            TiffCreateOptions::new()
                .with_gps_coordinates("0", "0")
                .with_gps_date("2026:02:29"),
        ];
        for options in invalid_options {
            assert!(matches!(
                create_tiff_to_vec(&options, ParseLimits::default()),
                Err(MetraError::InvalidTag { .. })
            ));
        }
    }

    #[test]
    fn rejects_unknown_duplicate_and_nul_entries() {
        let unknown = TiffCreateOptions::new().with_ascii("EXIF:Rating", "5");
        assert!(matches!(
            create_tiff_to_vec(&unknown, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let duplicate = TiffCreateOptions::new()
            .with_ascii("Make", "one")
            .with_ascii("EXIF:Make", "two");
        assert!(matches!(
            create_tiff_to_vec(&duplicate, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));

        let nul = TiffCreateOptions::new().with_ascii("Make", "bad\0value");
        assert!(matches!(
            create_tiff_to_vec(&nul, ParseLimits::default()),
            Err(MetraError::InvalidTag { .. })
        ));
    }

    #[test]
    fn enforces_creation_limits() {
        let options = TiffCreateOptions::new().with_ascii("Make", "Canon");
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        assert!(matches!(
            create_tiff_to_vec(&options, limits),
            Err(MetraError::ResourceLimitExceeded { .. })
        ));
    }

    #[test]
    fn creates_new_path_without_overwriting_existing_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("metra-tiff-create-{unique}"));
        fs::create_dir(&directory).expect("temporary directory should be created");
        let path = directory.join("created.tif");
        let options = TiffCreateOptions::new().with_ascii("Software", "Metra");

        create_tiff_path(&path, &options, ParseLimits::default())
            .expect("new TIFF path should be created");
        assert!(path.is_file());
        assert!(matches!(
            create_tiff_path(&path, &options, ParseLimits::default()),
            Err(MetraError::WriteFailure { .. })
        ));

        fs::remove_file(&path).expect("created TIFF should be removed");
        fs::remove_dir(&directory).expect("temporary directory should be removed");
    }
}

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

const PSD_HEADER_LENGTH: usize = 26;

pub fn read_psd<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < PSD_HEADER_LENGTH as u64 {
        return Err(MetraError::InvalidHeader {
            context: "PSD".to_owned(),
            message: "file is shorter than the PSD header".to_owned(),
        });
    }
    let header = read_at(
        reader,
        0,
        PSD_HEADER_LENGTH,
        file_length,
        &path,
        "PSD header",
    )?;
    if &header[..4] != b"8BPS" {
        return Err(MetraError::InvalidHeader {
            context: "PSD".to_owned(),
            message: "expected 8BPS signature".to_owned(),
        });
    }
    let version = u16::from_be_bytes([header[4], header[5]]);
    let format_name = match version {
        1 => "PSD",
        2 => "PSB",
        _ => {
            return Err(MetraError::InvalidHeader {
                context: "PSD".to_owned(),
                message: format!("unsupported Photoshop version {version}"),
            });
        }
    };
    add_header_tag(
        &mut metadata,
        "Format",
        TagValue::String(format_name.to_owned()),
        ValueType::String,
        4,
        2,
    );
    add_header_tag(
        &mut metadata,
        "Version",
        TagValue::Unsigned(u64::from(version)),
        ValueType::UnsignedInteger,
        4,
        2,
    );
    add_header_tag(
        &mut metadata,
        "NumChannels",
        TagValue::Unsigned(u64::from(read_u16(&header, 12))),
        ValueType::UnsignedInteger,
        12,
        2,
    );
    add_header_tag(
        &mut metadata,
        "ImageHeight",
        TagValue::Unsigned(u64::from(read_u32(&header, 14))),
        ValueType::UnsignedInteger,
        14,
        4,
    );
    add_header_tag(
        &mut metadata,
        "ImageWidth",
        TagValue::Unsigned(u64::from(read_u32(&header, 18))),
        ValueType::UnsignedInteger,
        18,
        4,
    );
    add_header_tag(
        &mut metadata,
        "BitDepth",
        TagValue::Unsigned(u64::from(read_u16(&header, 22))),
        ValueType::UnsignedInteger,
        22,
        2,
    );
    add_header_tag(
        &mut metadata,
        "ColorMode",
        TagValue::String(color_mode_name(read_u16(&header, 24)).to_owned()),
        ValueType::String,
        24,
        2,
    );

    let mut cursor = PSD_HEADER_LENGTH as u64;
    let color_mode_length = u64::from(read_u32_at(
        reader,
        cursor,
        file_length,
        &path,
        "PSD color mode length",
    )?);
    cursor = cursor.checked_add(4).ok_or(MetraError::InvalidOffset {
        context: "PSD color mode section".to_owned(),
        offset: cursor,
    })?;
    cursor = section_end(cursor, color_mode_length, file_length, "PSD color mode")?;

    let resource_length = u64::from(read_u32_at(
        reader,
        cursor,
        file_length,
        &path,
        "PSD image resource length",
    )?);
    cursor = cursor.checked_add(4).ok_or(MetraError::InvalidOffset {
        context: "PSD image resources".to_owned(),
        offset: cursor,
    })?;
    let resource_offset = cursor;
    let resource_end = section_end(
        resource_offset,
        resource_length,
        file_length,
        "PSD image resources",
    )?;
    if resource_length <= u64::try_from(limits.max_metadata_bytes).unwrap_or(u64::MAX) {
        let resource_bytes = read_at(
            reader,
            resource_offset,
            usize::try_from(resource_length).map_err(|_| MetraError::ResourceLimitExceeded {
                resource: "PSD image resources".to_owned(),
                limit: limits.max_metadata_bytes,
            })?,
            file_length,
            &path,
            "PSD image resources",
        )?;
        parse_resources(&resource_bytes, resource_offset, &mut metadata, limits)?;
    } else {
        metadata.add_warning(
            Warning::new(
                "psd-resource-limit",
                format!(
                    "PSD image resource section contains {resource_length} bytes; it exceeds the metadata budget"
                ),
            )
            .at(resource_offset),
        );
    }
    cursor = resource_end;

    let layer_length = u64::from(read_u32_at(
        reader,
        cursor,
        file_length,
        &path,
        "PSD layer and mask length",
    )?);
    let layer_start = cursor.checked_add(4).ok_or(MetraError::InvalidOffset {
        context: "PSD layer and mask section".to_owned(),
        offset: cursor,
    })?;
    section_end(
        layer_start,
        layer_length,
        file_length,
        "PSD layer and mask section",
    )?;

    metadata.sort_tags();
    Ok(metadata)
}

fn parse_resources(
    bytes: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) -> Result<()> {
    let mut cursor = 0_usize;
    let mut blocks = 0_usize;
    while cursor < bytes.len() {
        if blocks >= limits.max_jpeg_segments {
            metadata.add_warning(
                Warning::new(
                    "psd-resource-count-limit",
                    format!(
                        "stopped after {} Photoshop image resources",
                        limits.max_jpeg_segments
                    ),
                )
                .at(data_offset.saturating_add(cursor as u64)),
            );
            break;
        }
        if bytes.len().saturating_sub(cursor) < 8 {
            metadata.add_warning(
                Warning::new(
                    "truncated-psd-resource",
                    "Photoshop image resource header is truncated",
                )
                .at(data_offset.saturating_add(cursor as u64)),
            );
            break;
        }
        if &bytes[cursor..cursor + 4] != b"8BIM" && &bytes[cursor..cursor + 4] != b"8B64" {
            metadata.add_warning(
                Warning::new(
                    "invalid-psd-resource-signature",
                    "expected 8BIM or 8B64 image resource signature",
                )
                .at(data_offset.saturating_add(cursor as u64)),
            );
            break;
        }
        let resource_id = u16::from_be_bytes([bytes[cursor + 4], bytes[cursor + 5]]);
        let name_length = usize::from(bytes[cursor + 6]);
        let name_start = cursor + 7;
        let name_end = name_start
            .checked_add(name_length)
            .ok_or(MetraError::InvalidOffset {
                context: "PSD resource name".to_owned(),
                offset: name_start as u64,
            })?;
        let padded_name_end = name_end
            .checked_add((name_end - name_start + 1) % 2)
            .ok_or(MetraError::InvalidOffset {
                context: "PSD resource name padding".to_owned(),
                offset: name_end as u64,
            })?;
        let size_end = padded_name_end
            .checked_add(4)
            .ok_or(MetraError::InvalidOffset {
                context: "PSD resource size".to_owned(),
                offset: padded_name_end as u64,
            })?;
        if size_end > bytes.len() {
            metadata.add_warning(
                Warning::new("truncated-psd-resource", "resource size is truncated")
                    .at(data_offset.saturating_add(cursor as u64)),
            );
            break;
        }
        let resource_length = usize::try_from(u32::from_be_bytes(
            bytes[padded_name_end..size_end]
                .try_into()
                .expect("resource size"),
        ))
        .unwrap_or(usize::MAX);
        let resource_start = size_end;
        let resource_end =
            resource_start
                .checked_add(resource_length)
                .ok_or(MetraError::InvalidOffset {
                    context: format!("PSD resource 0x{resource_id:04X}"),
                    offset: resource_length as u64,
                })?;
        if resource_end > bytes.len() {
            metadata.add_warning(
                Warning::new(
                    "truncated-psd-resource",
                    format!("resource 0x{resource_id:04X} extends beyond the resource section"),
                )
                .at(data_offset.saturating_add(resource_start as u64)),
            );
            break;
        }
        let resource = &bytes[resource_start..resource_end];
        let resource_offset = data_offset.saturating_add(resource_start as u64);
        if resource.len() > limits.max_value_bytes {
            metadata.add_warning(
                Warning::new(
                    "psd-resource-value-limit",
                    format!("resource 0x{resource_id:04X} exceeds the value budget"),
                )
                .at(resource_offset),
            );
        } else {
            parse_resource(resource_id, resource, resource_offset, metadata, limits);
        }
        let padded_resource_end =
            resource_end
                .checked_add(resource.len() % 2)
                .ok_or(MetraError::InvalidOffset {
                    context: "PSD resource padding".to_owned(),
                    offset: resource_end as u64,
                })?;
        if padded_resource_end > bytes.len() {
            metadata.add_warning(
                Warning::new("truncated-psd-resource", "resource padding is truncated")
                    .at(data_offset.saturating_add(resource_end as u64)),
            );
            break;
        }
        cursor = padded_resource_end;
        blocks += 1;
    }
    Ok(())
}

fn parse_resource(
    resource_id: u16,
    resource: &[u8],
    data_offset: u64,
    metadata: &mut Metadata,
    limits: ParseLimits,
) {
    match resource_id {
        0x03ED => parse_resolution(resource, data_offset, metadata),
        0x0404 => {
            if let Err(error) =
                crate::iptc::parse_iptc_resource(resource, data_offset, metadata, limits)
            {
                metadata.add_warning(
                    Warning::new("invalid-psd-iptc", error.to_string()).at(data_offset),
                );
            }
        }
        0x040A => add_resource_value(
            metadata,
            resource_id,
            "CopyrightFlag",
            TagValue::Unsigned(u64::from(resource.first().copied().unwrap_or(0))),
            ValueType::UnsignedInteger,
            resource,
            data_offset,
        ),
        0x040B => add_resource_value(
            metadata,
            resource_id,
            "URL",
            TagValue::String(
                String::from_utf8_lossy(resource)
                    .trim_end_matches('\0')
                    .to_owned(),
            ),
            ValueType::String,
            resource,
            data_offset,
        ),
        0x040D => add_u32_resource(metadata, resource_id, "GlobalAngle", resource, data_offset),
        0x040F => match crate::icc::parse_icc_profile(resource, data_offset, metadata, limits) {
            Ok(()) => {}
            Err(error) => metadata
                .add_warning(Warning::new("invalid-psd-icc", error.to_string()).at(data_offset)),
        },
        0x0419 => add_u32_resource(
            metadata,
            resource_id,
            "GlobalAltitude",
            resource,
            data_offset,
        ),
        0x0422 => {
            let mut cursor = std::io::Cursor::new(resource);
            if let Err(error) = crate::tiff::parse_tiff_from_reader(
                &mut cursor,
                0,
                resource.len() as u64,
                data_offset,
                metadata,
                limits,
            ) {
                metadata.add_warning(
                    Warning::new("invalid-psd-exif", error.to_string()).at(data_offset),
                );
            }
        }
        0x0424 => {
            if resource.iter().all(|byte| *byte == 0) {
                return;
            }
            if let Err(error) =
                crate::xmp::parse_xmp(resource, data_offset, "PSD/XMP", metadata, limits)
            {
                metadata.add_warning(
                    Warning::new("invalid-psd-xmp", error.to_string()).at(data_offset),
                );
            }
        }
        _ => add_resource_value(
            metadata,
            resource_id,
            &format!("Resource0x{resource_id:04X}"),
            TagValue::Bytes(resource.to_vec()),
            ValueType::Bytes,
            resource,
            data_offset,
        ),
    }
}

fn parse_resolution(bytes: &[u8], data_offset: u64, metadata: &mut Metadata) {
    if bytes.len() < 16 {
        metadata.add_warning(
            Warning::new(
                "truncated-psd-resolution",
                "ResolutionInfo resource is shorter than 16 bytes",
            )
            .at(data_offset),
        );
        return;
    }
    add_resource_value(
        metadata,
        0x03ED,
        "HorizontalResolution",
        TagValue::Float(
            f64::from(u32::from_be_bytes(
                bytes[..4].try_into().expect("resolution"),
            )) / 65_536.0,
        ),
        ValueType::Float,
        &bytes[..4],
        data_offset,
    );
    add_resource_value(
        metadata,
        0x03ED,
        "HorizontalResolutionUnit",
        TagValue::Unsigned(u64::from(u16::from_be_bytes(
            bytes[4..6].try_into().expect("unit"),
        ))),
        ValueType::UnsignedInteger,
        &bytes[4..6],
        data_offset + 4,
    );
    add_resource_value(
        metadata,
        0x03ED,
        "VerticalResolution",
        TagValue::Float(
            f64::from(u32::from_be_bytes(
                bytes[8..12].try_into().expect("resolution"),
            )) / 65_536.0,
        ),
        ValueType::Float,
        &bytes[8..12],
        data_offset + 8,
    );
    add_resource_value(
        metadata,
        0x03ED,
        "VerticalResolutionUnit",
        TagValue::Unsigned(u64::from(u16::from_be_bytes(
            bytes[12..14].try_into().expect("unit"),
        ))),
        ValueType::UnsignedInteger,
        &bytes[12..14],
        data_offset + 12,
    );
}

fn add_u32_resource(
    metadata: &mut Metadata,
    resource_id: u16,
    name: &str,
    resource: &[u8],
    data_offset: u64,
) {
    if resource.len() != 4 {
        add_resource_value(
            metadata,
            resource_id,
            &format!("Resource0x{resource_id:04X}"),
            TagValue::Bytes(resource.to_vec()),
            ValueType::Bytes,
            resource,
            data_offset,
        );
        return;
    }
    add_resource_value(
        metadata,
        resource_id,
        name,
        TagValue::Unsigned(u64::from(u32::from_be_bytes(
            resource.try_into().expect("resource u32"),
        ))),
        ValueType::UnsignedInteger,
        resource,
        data_offset,
    );
}

fn add_resource_value(
    metadata: &mut Metadata,
    id: u16,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    raw_value: &[u8],
    offset: u64,
) {
    metadata.add_tag(Tag {
        namespace: "Photoshop".to_owned(),
        group: "ImageResources".to_owned(),
        id: Some(u32::from(id)),
        name: name.to_owned(),
        description: Some("Photoshop image resource".to_owned()),
        raw_value: Some(raw_value.to_vec()),
        value,
        value_type,
        source: Source::new(
            "PSD/ImageResources",
            Some(offset),
            Some(raw_value.len() as u64),
        ),
        writable: false,
    });
}

fn add_header_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "PSD".to_owned(),
        group: "Header".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("Photoshop document header property".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("PSD/Header", Some(offset), Some(length)),
        writable: false,
    });
}

fn color_mode_name(mode: u16) -> &'static str {
    match mode {
        0 => "Bitmap",
        1 => "Grayscale",
        2 => "Indexed",
        3 => "RGB",
        4 => "CMYK",
        7 => "Multichannel",
        8 => "Duotone",
        9 => "Lab",
        _ => "Unknown",
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("PSD u16"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("PSD u32"))
}

fn read_u32_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    file_length: u64,
    path: &Path,
    context: &str,
) -> Result<u32> {
    Ok(u32::from_be_bytes(
        read_at(reader, offset, 4, file_length, path, context)?
            .try_into()
            .expect("PSD u32"),
    ))
}

fn section_end(start: u64, length: u64, file_length: u64, context: &str) -> Result<u64> {
    let end = start.checked_add(length).ok_or(MetraError::InvalidOffset {
        context: context.to_owned(),
        offset: start,
    })?;
    if end > file_length {
        return Err(MetraError::UnexpectedEof {
            context: context.to_owned(),
        });
    }
    Ok(end)
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

    fn resource(id: u16, bytes: &[u8]) -> Vec<u8> {
        let mut output = b"8BIM".to_vec();
        output.extend_from_slice(&id.to_be_bytes());
        output.extend_from_slice(&[0, 0]);
        output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        output.extend_from_slice(bytes);
        if bytes.len() % 2 == 1 {
            output.push(0);
        }
        output
    }

    fn psd(resources: &[u8]) -> Vec<u8> {
        let mut output = vec![0_u8; PSD_HEADER_LENGTH];
        output[..4].copy_from_slice(b"8BPS");
        output[4..6].copy_from_slice(&1_u16.to_be_bytes());
        output[12..14].copy_from_slice(&3_u16.to_be_bytes());
        output[14..18].copy_from_slice(&100_u32.to_be_bytes());
        output[18..22].copy_from_slice(&200_u32.to_be_bytes());
        output[22..24].copy_from_slice(&8_u16.to_be_bytes());
        output[24..26].copy_from_slice(&3_u16.to_be_bytes());
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&(resources.len() as u32).to_be_bytes());
        output.extend_from_slice(resources);
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&0_u16.to_be_bytes());
        output
    }

    fn exif_resource() -> Vec<u8> {
        let mut bytes = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
        bytes.extend_from_slice(&0x010F_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&6_u32.to_le_bytes());
        bytes.extend_from_slice(&26_u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(b"Canon\0");
        bytes
    }

    fn icc_resource() -> Vec<u8> {
        let mut bytes = vec![0_u8; 132];
        bytes[..4].copy_from_slice(&132_u32.to_be_bytes());
        bytes[8..10].copy_from_slice(&[4, 0x30]);
        bytes[16..20].copy_from_slice(b"RGB ");
        bytes[36..40].copy_from_slice(b"acsp");
        bytes
    }

    #[test]
    fn reads_psd_header_and_common_image_resources() {
        let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"/>"#;
        let resources = [
            resource(0x0424, xmp),
            resource(0x040B, b"https://metra.test"),
        ]
        .concat();
        let bytes = psd(&resources);
        let info = FileInfo::new(
            "design.psd".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Psd,
        );
        let metadata = read_psd(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("PSD fixture should parse");

        assert_eq!(metadata.file_info.format, metra_core::FileFormat::Psd);
        assert_eq!(
            metadata.find("PSD:ImageWidth").unwrap().value,
            TagValue::Unsigned(200)
        );
        assert_eq!(
            metadata.find("PSD:ColorMode").unwrap().display_value(),
            "RGB"
        );
        assert!(metadata.find("XMP:Packet").is_some());
        assert_eq!(
            metadata.find("Photoshop:URL").unwrap().value,
            TagValue::String("https://metra.test".to_owned())
        );
    }

    #[test]
    fn delegates_embedded_iptc_icc_and_exif_resources() {
        let iptc = [0x1C, 2, 25, 0, 7]
            .into_iter()
            .chain(b"keyword".iter().copied())
            .collect::<Vec<_>>();
        let bytes = psd(&[
            resource(0x0404, &iptc),
            resource(0x040F, &icc_resource()),
            resource(0x0422, &exif_resource()),
        ]
        .concat());
        let info = FileInfo::new(
            "embedded.psd".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Psd,
        );
        let metadata = read_psd(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect("embedded PSD resources should parse");

        assert_eq!(
            metadata.find("IPTC:Keywords").unwrap().value,
            TagValue::String("keyword".to_owned())
        );
        assert_eq!(
            metadata.find("ICC:ColorSpace").unwrap().value,
            TagValue::String("RGB".to_owned())
        );
        assert_eq!(
            metadata.find("EXIF:Make").unwrap().value,
            TagValue::String("Canon".to_owned())
        );
    }

    #[test]
    fn refuses_truncated_psd_sections() {
        let mut bytes = vec![0_u8; PSD_HEADER_LENGTH];
        bytes[..4].copy_from_slice(b"8BPS");
        bytes[4..6].copy_from_slice(&1_u16.to_be_bytes());
        bytes.extend_from_slice(&64_u32.to_be_bytes());
        let info = FileInfo::new(
            "truncated.psd".into(),
            bytes.len() as u64,
            metra_core::FileFormat::Psd,
        );
        let error = read_psd(&mut Cursor::new(bytes), info, ParseLimits::default())
            .expect_err("truncated PSD should fail");
        assert!(matches!(error, MetraError::UnexpectedEof { .. }));
    }
}

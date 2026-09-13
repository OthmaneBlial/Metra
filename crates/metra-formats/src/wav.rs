use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};
use quick_xml::Reader;
use quick_xml::events::Event;

pub fn read_wav<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let file_length = file_info.size;
    let mut metadata = Metadata::new(file_info);
    if file_length < 12 {
        return Err(MetraError::InvalidHeader {
            context: "WAV".to_owned(),
            message: "file is shorter than the RIFF/WAVE header".to_owned(),
        });
    }
    let header = read_at(reader, 0, 12, &path, "WAV header")?;
    if &header[..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(MetraError::InvalidHeader {
            context: "WAV".to_owned(),
            message: "expected RIFF/WAVE signature".to_owned(),
        });
    }
    let declared_size = u64::from(u32::from_le_bytes(
        header[4..8].try_into().expect("RIFF size"),
    ));
    let declared_end = 8_u64.saturating_add(declared_size);
    let parse_end = declared_end.min(file_length);
    if declared_end > file_length {
        metadata.add_warning(Warning::new(
            "truncated-riff",
            "RIFF declared size extends beyond the file",
        ));
    }
    add_tag(
        &mut metadata,
        "ContainerSize",
        TagValue::Unsigned(declared_size),
        ValueType::UnsignedInteger,
        "RIFF",
        4,
        4,
    );

    let mut offset = 12_u64;
    let mut chunk_count = 0_usize;
    let mut metadata_bytes = 0_usize;
    while offset < parse_end {
        if chunk_count >= limits.max_jpeg_segments {
            metadata.add_warning(Warning::new(
                "wav-chunk-limit",
                format!("stopped after {} WAV chunks", limits.max_jpeg_segments),
            ));
            break;
        }
        if parse_end.saturating_sub(offset) < 8 {
            metadata.add_warning(
                Warning::new("truncated-wav-chunk", "WAV chunk header is truncated").at(offset),
            );
            break;
        }
        let chunk_header = read_at(reader, offset, 8, &path, "WAV chunk header")?;
        let kind: [u8; 4] = chunk_header[..4].try_into().expect("WAV chunk id");
        let length = u64::from(u32::from_le_bytes(
            chunk_header[4..8].try_into().expect("WAV chunk size"),
        ));
        let data_offset = offset + 8;
        let data_end = data_offset
            .checked_add(length)
            .ok_or(MetraError::InvalidOffset {
                context: format!("WAV {} chunk", fourcc(&kind)),
                offset: data_offset,
            })?;
        if data_end > parse_end {
            metadata.add_warning(
                Warning::new(
                    "truncated-wav-chunk",
                    format!(
                        "WAV {} chunk extends beyond the RIFF boundary",
                        fourcc(&kind)
                    ),
                )
                .at(data_offset),
            );
            break;
        }
        let padded_end = data_end
            .checked_add(length & 1)
            .ok_or(MetraError::InvalidOffset {
                context: format!("WAV {} padding", fourcc(&kind)),
                offset: data_end,
            })?;
        if padded_end > file_length {
            metadata.add_warning(
                Warning::new("truncated-wav-padding", "WAV chunk padding is truncated")
                    .at(data_end),
            );
            break;
        }
        let kind_name = fourcc(&kind);
        match &kind {
            b"fmt " => {
                if length < 16 {
                    metadata.add_warning(
                        Warning::new("invalid-wav-format", "fmt chunk is shorter than 16 bytes")
                            .at(data_offset),
                    );
                } else {
                    let bounded = read_bounded(
                        reader,
                        data_offset,
                        length,
                        &path,
                        limits,
                        &mut metadata_bytes,
                        "WAV fmt chunk",
                    )?;
                    if let Some(data) = bounded {
                        parse_fmt(&data, data_offset, &mut metadata);
                    } else {
                        metadata.add_warning(
                            Warning::new(
                                "wav-metadata-limit",
                                "WAV fmt chunk was skipped due to limits",
                            )
                            .at(data_offset),
                        );
                    }
                }
            }
            b"LIST" => {
                let bounded = read_bounded(
                    reader,
                    data_offset,
                    length,
                    &path,
                    limits,
                    &mut metadata_bytes,
                    "WAV LIST chunk",
                )?;
                if let Some(data) = bounded {
                    parse_list_info(&data, data_offset, limits, &mut metadata);
                } else {
                    metadata.add_warning(
                        Warning::new(
                            "wav-metadata-limit",
                            "WAV LIST chunk was skipped due to limits",
                        )
                        .at(data_offset),
                    );
                }
            }
            b"bext" => {
                let bounded = read_bounded(
                    reader,
                    data_offset,
                    length,
                    &path,
                    limits,
                    &mut metadata_bytes,
                    "WAV bext chunk",
                )?;
                if let Some(data) = bounded {
                    parse_bext(&data, data_offset, &mut metadata);
                } else {
                    metadata.add_warning(
                        Warning::new(
                            "wav-metadata-limit",
                            "WAV bext chunk was skipped due to limits",
                        )
                        .at(data_offset),
                    );
                }
            }
            b"fact" => {
                if length >= 4 {
                    let data = read_at(reader, data_offset, 4, &path, "WAV fact chunk")?;
                    add_tag(
                        &mut metadata,
                        "SampleLength",
                        TagValue::Unsigned(u64::from(u32::from_le_bytes(
                            data.try_into().expect("WAV fact sample length"),
                        ))),
                        ValueType::UnsignedInteger,
                        "fact",
                        data_offset,
                        4,
                    );
                }
            }
            b"iXML" => {
                let bounded = read_bounded(
                    reader,
                    data_offset,
                    length,
                    &path,
                    limits,
                    &mut metadata_bytes,
                    "WAV iXML chunk",
                )?;
                if let Some(data) = bounded {
                    parse_ixml(&data, data_offset, limits, &mut metadata);
                } else {
                    metadata.add_warning(
                        Warning::new(
                            "wav-metadata-limit",
                            "WAV iXML chunk was skipped due to limits",
                        )
                        .at(data_offset),
                    );
                }
            }
            b"id3 " => {
                metadata.add_warning(
                    Warning::new("wav-id3", "WAV ID3 chunk is present but not decoded")
                        .at(data_offset),
                );
            }
            b"data" => {}
            _ => {
                if length > limits.max_value_bytes as u64 {
                    metadata.add_warning(
                        Warning::new(
                            "wav-unsupported-chunk",
                            format!("WAV {kind_name} chunk was skipped because it is too large"),
                        )
                        .at(data_offset),
                    );
                }
            }
        }
        offset = padded_end;
        chunk_count += 1;
    }
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_fmt(data: &[u8], offset: u64, metadata: &mut Metadata) {
    let audio_format = u16::from_le_bytes([data[0], data[1]]);
    let channels = u16::from_le_bytes([data[2], data[3]]);
    let sample_rate = u32::from_le_bytes(data[4..8].try_into().expect("WAV sample rate"));
    let byte_rate = u32::from_le_bytes(data[8..12].try_into().expect("WAV byte rate"));
    let block_align = u16::from_le_bytes(data[12..14].try_into().expect("WAV block align"));
    let bits_per_sample = u16::from_le_bytes(data[14..16].try_into().expect("WAV bits per sample"));
    add_tag(
        metadata,
        "AudioFormat",
        TagValue::Unsigned(u64::from(audio_format)),
        ValueType::UnsignedInteger,
        "fmt",
        offset,
        2,
    );
    add_tag(
        metadata,
        "Channels",
        TagValue::Unsigned(u64::from(channels)),
        ValueType::UnsignedInteger,
        "fmt",
        offset + 2,
        2,
    );
    add_tag(
        metadata,
        "SampleRateHz",
        TagValue::Unsigned(u64::from(sample_rate)),
        ValueType::UnsignedInteger,
        "fmt",
        offset + 4,
        4,
    );
    add_tag(
        metadata,
        "ByteRate",
        TagValue::Unsigned(u64::from(byte_rate)),
        ValueType::UnsignedInteger,
        "fmt",
        offset + 8,
        4,
    );
    add_tag(
        metadata,
        "BlockAlign",
        TagValue::Unsigned(u64::from(block_align)),
        ValueType::UnsignedInteger,
        "fmt",
        offset + 12,
        2,
    );
    add_tag(
        metadata,
        "BitsPerSample",
        TagValue::Unsigned(u64::from(bits_per_sample)),
        ValueType::UnsignedInteger,
        "fmt",
        offset + 14,
        2,
    );
    if audio_format == 0xFFFE && data.len() >= 40 {
        let valid_bits = u16::from_le_bytes([data[18], data[19]]);
        let channel_mask = u32::from_le_bytes(data[20..24].try_into().expect("WAV channel mask"));
        add_tag(
            metadata,
            "ValidBitsPerSample",
            TagValue::Unsigned(u64::from(valid_bits)),
            ValueType::UnsignedInteger,
            "fmt",
            offset + 18,
            2,
        );
        add_tag(
            metadata,
            "ChannelMask",
            TagValue::Unsigned(u64::from(channel_mask)),
            ValueType::UnsignedInteger,
            "fmt",
            offset + 20,
            4,
        );
    }
}

fn parse_ixml(data: &[u8], offset: u64, limits: ParseLimits, metadata: &mut Metadata) {
    metadata.add_tag(Tag {
        namespace: "WAV".to_owned(),
        group: "iXML".to_owned(),
        id: None,
        name: "iXML:Packet".to_owned(),
        description: Some("Raw bounded WAV iXML packet".to_owned()),
        raw_value: Some(data.to_vec()),
        value: TagValue::Bytes(data.to_vec()),
        value_type: ValueType::Bytes,
        source: Source::new("WAV/iXML", Some(offset), Some(data.len() as u64)),
        writable: false,
    });

    let mut reader = Reader::from_reader(data);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut element_count = 0_usize;
    let mut text_bytes = 0_usize;
    loop {
        let event = match reader.read_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                metadata
                    .add_warning(Warning::new("invalid-wav-ixml", error.to_string()).at(offset));
                return;
            }
        };
        match event {
            Event::Start(element) => {
                if stack.len() >= limits.max_recursion_depth {
                    metadata.add_warning(
                        Warning::new("wav-ixml-recursion-limit", "WAV iXML nesting limit reached")
                            .at(offset),
                    );
                    return;
                }
                element_count = element_count.saturating_add(1);
                if element_count > limits.max_jpeg_segments {
                    metadata.add_warning(
                        Warning::new("wav-ixml-element-limit", "WAV iXML element limit reached")
                            .at(offset),
                    );
                    return;
                }
                let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
                stack.push(name);
                texts.push(String::new());
            }
            Event::Empty(_element) => {
                element_count = element_count.saturating_add(1);
                if element_count > limits.max_jpeg_segments {
                    metadata.add_warning(
                        Warning::new("wav-ixml-element-limit", "WAV iXML element limit reached")
                            .at(offset),
                    );
                    return;
                }
            }
            Event::Text(text) => {
                let decoded = match text.decode() {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        metadata.add_warning(
                            Warning::new("invalid-wav-ixml", error.to_string()).at(offset),
                        );
                        return;
                    }
                };
                let unescaped = match quick_xml::escape::unescape(decoded.as_ref()) {
                    Ok(value) => value,
                    Err(error) => {
                        metadata.add_warning(
                            Warning::new("invalid-wav-ixml", error.to_string()).at(offset),
                        );
                        return;
                    }
                };
                text_bytes = text_bytes.saturating_add(unescaped.len());
                if text_bytes > limits.max_value_bytes {
                    metadata.add_warning(
                        Warning::new("wav-ixml-value-limit", "WAV iXML text budget was reached")
                            .at(offset),
                    );
                    return;
                }
                if let Some(value) = texts.last_mut() {
                    value.push_str(unescaped.as_ref());
                }
            }
            Event::CData(text) => {
                let decoded = match text.decode() {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        metadata.add_warning(
                            Warning::new("invalid-wav-ixml", error.to_string()).at(offset),
                        );
                        return;
                    }
                };
                text_bytes = text_bytes.saturating_add(decoded.len());
                if text_bytes > limits.max_value_bytes {
                    metadata.add_warning(
                        Warning::new("wav-ixml-value-limit", "WAV iXML text budget was reached")
                            .at(offset),
                    );
                    return;
                }
                if let Some(value) = texts.last_mut() {
                    value.push_str(decoded.as_ref());
                }
            }
            Event::End(element) => {
                let Some(name) = stack.pop() else {
                    metadata.add_warning(
                        Warning::new("invalid-wav-ixml", "unexpected WAV iXML closing element")
                            .at(offset),
                    );
                    return;
                };
                let Some(value) = texts.pop() else {
                    metadata.add_warning(
                        Warning::new("invalid-wav-ixml", "WAV iXML element stack is inconsistent")
                            .at(offset),
                    );
                    return;
                };
                if element.name().as_ref() != name.as_bytes() {
                    metadata.add_warning(
                        Warning::new(
                            "invalid-wav-ixml",
                            "WAV iXML closing element does not match",
                        )
                        .at(offset),
                    );
                    return;
                }
                if !value.is_empty() {
                    let path = stack
                        .iter()
                        .chain(std::iter::once(&name))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(".");
                    metadata.add_tag(Tag {
                        namespace: "WAV".to_owned(),
                        group: "iXML".to_owned(),
                        id: None,
                        name: format!("iXML:{path}"),
                        description: Some("WAV iXML leaf value".to_owned()),
                        raw_value: Some(value.as_bytes().to_vec()),
                        value: TagValue::String(value),
                        value_type: ValueType::String,
                        source: Source::new("WAV/iXML", Some(offset), Some(data.len() as u64)),
                        writable: false,
                    });
                }
            }
            Event::DocType(_) => {
                metadata.add_warning(
                    Warning::new("invalid-wav-ixml", "DOCTYPE is not allowed in WAV iXML")
                        .at(offset),
                );
                return;
            }
            Event::Eof => break,
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::GeneralRef(_) => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        metadata.add_warning(
            Warning::new("invalid-wav-ixml", "WAV iXML ended with unclosed elements").at(offset),
        );
    }
}

fn parse_list_info(data: &[u8], offset: u64, limits: ParseLimits, metadata: &mut Metadata) {
    if data.len() < 4 || &data[..4] != b"INFO" {
        return;
    }
    let mut cursor = 4_usize;
    while cursor < data.len() {
        if data.len().saturating_sub(cursor) < 8 {
            metadata.add_warning(
                Warning::new(
                    "truncated-wav-info",
                    "LIST/INFO subchunk header is truncated",
                )
                .at(offset + cursor as u64),
            );
            return;
        }
        let kind: [u8; 4] = data[cursor..cursor + 4]
            .try_into()
            .expect("WAV INFO subchunk id");
        let length = u32::from_le_bytes(
            data[cursor + 4..cursor + 8]
                .try_into()
                .expect("WAV INFO subchunk size"),
        ) as usize;
        let value_start = cursor + 8;
        let Some(value_end) = value_start.checked_add(length) else {
            metadata.add_warning(
                Warning::new("invalid-wav-info", "LIST/INFO subchunk size overflows")
                    .at(offset + value_start as u64),
            );
            return;
        };
        if value_end > data.len() {
            metadata.add_warning(
                Warning::new(
                    "truncated-wav-info",
                    "LIST/INFO value exceeds its parent chunk",
                )
                .at(offset + value_start as u64),
            );
            return;
        }
        if length <= limits.max_value_bytes {
            if let Some(name) = info_name(&kind) {
                let value = String::from_utf8_lossy(&data[value_start..value_end])
                    .trim_end_matches('\0')
                    .to_owned();
                if !value.is_empty() {
                    add_tag(
                        metadata,
                        name,
                        TagValue::String(value),
                        ValueType::String,
                        "LIST/INFO",
                        offset + value_start as u64,
                        length as u64,
                    );
                }
            }
        } else {
            metadata.add_warning(
                Warning::new(
                    "wav-info-limit",
                    "LIST/INFO value exceeded the value budget",
                )
                .at(offset + value_start as u64),
            );
        }
        cursor = value_end + (length & 1);
    }
}

fn parse_bext(data: &[u8], offset: u64, metadata: &mut Metadata) {
    let fields = [
        ("Description", 0_usize, 256_usize),
        ("Originator", 256, 32),
        ("OriginatorReference", 288, 32),
        ("OriginationDate", 320, 10),
        ("OriginationTime", 330, 8),
    ];
    for (name, start, length) in fields {
        let Some(value) = data.get(start..start + length) else {
            return;
        };
        let value = String::from_utf8_lossy(value)
            .trim_end_matches('\0')
            .trim_end()
            .to_owned();
        if !value.is_empty() {
            add_tag(
                metadata,
                name,
                TagValue::String(value),
                ValueType::String,
                "bext",
                offset + start as u64,
                length as u64,
            );
        }
    }
    if data.len() >= 348 {
        let time_reference =
            u64::from_le_bytes(data[338..346].try_into().expect("BWF time reference"));
        let version = u16::from_le_bytes(data[346..348].try_into().expect("BWF version"));
        add_tag(
            metadata,
            "TimeReference",
            TagValue::Unsigned(time_reference),
            ValueType::UnsignedInteger,
            "bext",
            offset + 338,
            8,
        );
        add_tag(
            metadata,
            "Version",
            TagValue::Unsigned(u64::from(version)),
            ValueType::UnsignedInteger,
            "bext",
            offset + 346,
            2,
        );
    }
}

fn info_name(kind: &[u8; 4]) -> Option<&'static str> {
    Some(match kind {
        b"INAM" => "Title",
        b"IART" => "Artist",
        b"IPRD" => "Product",
        b"ICMT" => "Comment",
        b"ICRD" => "CreationDate",
        b"IGNR" => "Genre",
        b"IENG" => "Engineer",
        b"ISFT" => "Software",
        b"ICOP" => "Copyright",
        b"ITCH" => "Technician",
        b"ISBJ" => "Subject",
        b"ISRC" => "Source",
        _ => return None,
    })
}

fn fourcc(bytes: &[u8; 4]) -> String {
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        String::from_utf8_lossy(bytes).to_string()
    } else {
        format!(
            "0x{:02X}{:02X}{:02X}{:02X}",
            bytes[0], bytes[1], bytes[2], bytes[3]
        )
    }
}

fn read_bounded<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: u64,
    path: &Path,
    limits: ParseLimits,
    metadata_bytes: &mut usize,
    context: &str,
) -> Result<Option<Vec<u8>>> {
    let length = usize::try_from(length).map_err(|_| MetraError::ResourceLimitExceeded {
        resource: context.to_owned(),
        limit: limits.max_value_bytes,
    })?;
    if length > limits.max_value_bytes {
        return Ok(None);
    }
    let next_total =
        metadata_bytes
            .checked_add(length)
            .ok_or(MetraError::ResourceLimitExceeded {
                resource: "WAV metadata".to_owned(),
                limit: limits.max_metadata_bytes,
            })?;
    if next_total > limits.max_metadata_bytes {
        return Ok(None);
    }
    *metadata_bytes = next_total;
    Ok(Some(read_at(reader, offset, length, path, context)?))
}

fn add_tag(
    metadata: &mut Metadata,
    name: &str,
    value: TagValue,
    value_type: ValueType,
    group: &str,
    offset: u64,
    length: u64,
) {
    metadata.add_tag(Tag {
        namespace: "WAV".to_owned(),
        group: group.to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("WAV metadata".to_owned()),
        raw_value: None,
        value,
        value_type,
        source: Source::new("WAV", Some(offset), Some(length)),
        writable: false,
    });
}

fn read_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    length: usize,
    path: &Path,
    context: &str,
) -> Result<Vec<u8>> {
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|source| io_error(path, source))?;
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| match source.kind() {
            std::io::ErrorKind::UnexpectedEof => MetraError::UnexpectedEof {
                context: context.to_owned(),
            },
            _ => io_error(path, source),
        })?;
    Ok(bytes)
}

fn io_error(path: &Path, source: std::io::Error) -> MetraError {
    if source.kind() == std::io::ErrorKind::UnexpectedEof {
        MetraError::UnexpectedEof {
            context: path.display().to_string(),
        }
    } else {
        MetraError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use metra_core::{FileFormat, FileInfo};

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut result = kind.to_vec();
        result.extend_from_slice(&(data.len() as u32).to_le_bytes());
        result.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            result.push(0);
        }
        result
    }

    #[test]
    fn reads_wav_format_and_info_metadata() {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&2_u16.to_le_bytes());
        fmt.extend_from_slice(&44_100_u32.to_le_bytes());
        fmt.extend_from_slice(&176_400_u32.to_le_bytes());
        fmt.extend_from_slice(&4_u16.to_le_bytes());
        fmt.extend_from_slice(&16_u16.to_le_bytes());
        let mut list = b"INFO".to_vec();
        list.extend(chunk(b"INAM", b"Metra\0"));
        list.extend(chunk(b"IART", b"Artist\0"));
        let mut body = chunk(b"fmt ", &fmt);
        body.extend(chunk(b"LIST", &list));
        body.extend(chunk(b"data", &[0, 0, 0, 0]));
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend(body);
        let info = FileInfo::new("track.wav".into(), bytes.len() as u64, FileFormat::Wav);
        let metadata = read_wav(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();
        assert_eq!(
            metadata.find("WAV:SampleRateHz").unwrap().display_value(),
            "44100"
        );
        assert_eq!(metadata.find("WAV:Title").unwrap().display_value(), "Metra");
        assert_eq!(
            metadata.find("WAV:Artist").unwrap().display_value(),
            "Artist"
        );
    }

    #[test]
    fn reads_bounded_ixml_leaf_values() {
        let ixml = br#"<BWFXML><PROJECT>Metra</PROJECT><SCENE><TAKE>07</TAKE></SCENE><NOTE><![CDATA[clean take]]></NOTE></BWFXML>"#;
        let mut body = chunk(b"iXML", ixml);
        body.extend(chunk(b"data", &[0, 0]));
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend(body);
        let info = FileInfo::new("ixml.wav".into(), bytes.len() as u64, FileFormat::Wav);
        let metadata = read_wav(&mut Cursor::new(bytes), info, ParseLimits::default()).unwrap();

        assert_eq!(
            metadata
                .find("WAV:iXML:BWFXML.PROJECT")
                .unwrap()
                .display_value(),
            "Metra"
        );
        assert_eq!(
            metadata
                .find("WAV:iXML:BWFXML.SCENE.TAKE")
                .unwrap()
                .display_value(),
            "07"
        );
        assert_eq!(
            metadata
                .find("WAV:iXML:BWFXML.NOTE")
                .unwrap()
                .display_value(),
            "clean take"
        );
        assert!(metadata.find("WAV:iXML:Packet").is_some());
        assert!(
            !metadata
                .warnings
                .iter()
                .any(|warning| warning.code == "wav-ixml")
        );
    }

    #[test]
    fn rejects_invalid_wave_header() {
        let bytes = b"RIFF\0\0\0\0NOPE";
        let info = FileInfo::new("bad.wav".into(), bytes.len() as u64, FileFormat::Wav);
        let result = read_wav(&mut Cursor::new(bytes), info, ParseLimits::default());
        assert!(matches!(result, Err(MetraError::InvalidHeader { .. })));
    }
}

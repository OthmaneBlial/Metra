use std::io::{Read, Seek};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use metra_core::{
    FileInfo, Metadata, MetraError, ParseLimits, Result, Source, Tag, TagValue, ValueType, Warning,
};

use crate::xml::resolve_general_ref;

#[derive(Debug)]
struct ElementState {
    name: String,
    text: String,
}

/// Read bounded document-level SVG metadata without rendering the image.
pub fn read_svg<R: Read + Seek>(
    reader: &mut R,
    file_info: FileInfo,
    limits: ParseLimits,
) -> Result<Metadata> {
    let path = file_info.path.clone();
    let length =
        usize::try_from(file_info.size).map_err(|_| MetraError::ResourceLimitExceeded {
            resource: "SVG document".to_owned(),
            limit: limits.max_metadata_bytes,
        })?;
    if length > limits.max_metadata_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "SVG document".to_owned(),
            limit: limits.max_metadata_bytes,
        });
    }
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|source| MetraError::Io {
            path: path.clone(),
            source,
        })?;

    let mut metadata = Metadata::new(file_info);
    parse_document(&bytes, limits, &mut metadata)?;
    metadata.sort_tags();
    Ok(metadata)
}

fn parse_document(bytes: &[u8], limits: ParseLimits, metadata: &mut Metadata) -> Result<()> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<ElementState>::new();
    let mut node_count = 0_usize;
    let mut text_bytes = 0_usize;
    let mut seen_root = false;
    let mut closed_root = false;
    let mut embedded_xmp: Option<(usize, usize)> = None;

    loop {
        let event =
            reader
                .read_event_into(&mut buffer)
                .map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
        match event {
            Event::Start(element) => {
                ensure_node_budget(&mut node_count, limits)?;
                if closed_root {
                    return Err(MetraError::InvalidXml {
                        message: "SVG contains content after its root element".to_owned(),
                    });
                }
                let name = local_name(element.name().as_ref());
                if embedded_xmp.is_none() && name == "xmpmeta" {
                    let end = reader.buffer_position() as usize;
                    let start = end.saturating_sub(element.as_ref().len().saturating_add(2));
                    embedded_xmp = Some((start, 1));
                } else if let Some((_, depth)) = embedded_xmp.as_mut() {
                    *depth = depth.saturating_add(1);
                }
                if !seen_root {
                    if name != "svg" {
                        return Err(MetraError::InvalidXml {
                            message: "SVG document must start with an <svg> root".to_owned(),
                        });
                    }
                    add_root_attributes(metadata, &element, &reader, bytes.len() as u64)?;
                    seen_root = true;
                }
                ensure_depth(stack.len(), limits)?;
                stack.push(ElementState {
                    name,
                    text: String::new(),
                });
            }
            Event::Empty(element) => {
                ensure_node_budget(&mut node_count, limits)?;
                if closed_root {
                    return Err(MetraError::InvalidXml {
                        message: "SVG contains content after its root element".to_owned(),
                    });
                }
                let name = local_name(element.name().as_ref());
                if !seen_root {
                    if name != "svg" {
                        return Err(MetraError::InvalidXml {
                            message: "SVG document must start with an <svg> root".to_owned(),
                        });
                    }
                    add_root_attributes(metadata, &element, &reader, bytes.len() as u64)?;
                    seen_root = true;
                    closed_root = true;
                }
            }
            Event::Text(text) => {
                let decoded = text.decode().map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
                let unescaped = quick_xml::escape::unescape(decoded.as_ref()).map_err(|error| {
                    MetraError::InvalidXml {
                        message: error.to_string(),
                    }
                })?;
                append_text(
                    stack.last_mut(),
                    unescaped.as_ref(),
                    &mut text_bytes,
                    limits,
                )?;
            }
            Event::CData(text) => {
                let decoded = text.decode().map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
                append_text(stack.last_mut(), decoded.as_ref(), &mut text_bytes, limits)?;
            }
            Event::End(end) => {
                let element = stack.pop().ok_or_else(|| MetraError::InvalidXml {
                    message: format!(
                        "unexpected closing element {}",
                        display_name(end.name().as_ref())
                    ),
                })?;
                let closing_name = local_name(end.name().as_ref());
                if element.name != closing_name {
                    return Err(MetraError::InvalidXml {
                        message: format!(
                            "closing element </{closing_name}> does not match <{}>",
                            element.name
                        ),
                    });
                }
                if element.name == "title" {
                    add_text_tag(
                        metadata,
                        "Title",
                        &element.text,
                        "SVG/Title",
                        bytes.len() as u64,
                    );
                } else if element.name == "desc" {
                    add_text_tag(
                        metadata,
                        "Description",
                        &element.text,
                        "SVG/Description",
                        bytes.len() as u64,
                    );
                }
                if let Some(parent) = stack.last_mut() {
                    parent.text.push_str(&element.text);
                } else {
                    closed_root = true;
                }
                if let Some((_, depth)) = embedded_xmp.as_mut() {
                    *depth = depth.saturating_sub(1);
                    if *depth == 0 {
                        let (start, _) = embedded_xmp
                            .take()
                            .expect("embedded XMP capture should still be present");
                        let end = reader.buffer_position() as usize;
                        if let Some(packet) = bytes.get(start..end) {
                            if let Err(error) = crate::xmp::parse_xmp(
                                packet,
                                start as u64,
                                "SVG/XMP",
                                metadata,
                                limits,
                            ) {
                                metadata.add_warning(
                                    Warning::new("invalid-svg-xmp", error.to_string())
                                        .at(start as u64),
                                );
                            }
                        } else {
                            metadata.add_warning(
                                Warning::new(
                                    "invalid-svg-xmp-range",
                                    "embedded SVG XMP range is outside the document",
                                )
                                .at(start as u64),
                            );
                        }
                    }
                }
            }
            Event::Comment(comment) => {
                let value = comment.decode().map_err(|error| MetraError::InvalidXml {
                    message: error.to_string(),
                })?;
                if !value.trim().is_empty() {
                    add_text_tag(
                        metadata,
                        "Comment",
                        value.as_ref(),
                        "SVG/Comment",
                        bytes.len() as u64,
                    );
                }
            }
            Event::DocType(_) => {
                return Err(MetraError::InvalidXml {
                    message: "DOCTYPE is not allowed in SVG metadata".to_owned(),
                });
            }
            Event::GeneralRef(reference) => {
                let value = resolve_general_ref(&reference)?;
                append_text(stack.last_mut(), &value, &mut text_bytes, limits)?;
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) => {}
        }
        buffer.clear();
    }

    if !seen_root {
        return Err(MetraError::InvalidXml {
            message: "SVG document has no root element".to_owned(),
        });
    }
    if !stack.is_empty() {
        return Err(MetraError::InvalidXml {
            message: "SVG document ended with unclosed elements".to_owned(),
        });
    }
    Ok(())
}

fn add_root_attributes(
    metadata: &mut Metadata,
    element: &BytesStart<'_>,
    reader: &Reader<&[u8]>,
    document_length: u64,
) -> Result<()> {
    let source = Source::new("SVG/Root", Some(0), Some(document_length));
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute.map_err(|error| MetraError::InvalidXml {
            message: error.to_string(),
        })?;
        let name = display_name(attribute.key.as_ref());
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| MetraError::InvalidXml {
                message: error.to_string(),
            })?
            .into_owned();
        let Some((tag_name, value, value_type)) = root_attribute(&name, &value) else {
            continue;
        };
        metadata.add_tag(Tag {
            namespace: "SVG".to_owned(),
            group: "Root".to_owned(),
            id: None,
            name: tag_name.to_owned(),
            description: Some("SVG root document property".to_owned()),
            raw_value: Some(value.to_display_string().into_bytes()),
            value,
            value_type,
            source: source.clone(),
            writable: false,
        });
    }
    Ok(())
}

fn root_attribute(name: &str, value: &str) -> Option<(&'static str, TagValue, ValueType)> {
    match name {
        "width" => Some((
            "Width",
            TagValue::String(value.to_owned()),
            ValueType::String,
        )),
        "height" => Some((
            "Height",
            TagValue::String(value.to_owned()),
            ValueType::String,
        )),
        "version" => Some((
            "Version",
            TagValue::String(value.to_owned()),
            ValueType::String,
        )),
        "viewBox" => {
            let values = value
                .split(|character: char| character.is_ascii_whitespace() || character == ',')
                .filter(|item| !item.is_empty())
                .map(str::parse::<f64>)
                .collect::<std::result::Result<Vec<_>, _>>()
                .ok()?;
            if values.len() == 4 && values.iter().all(|item| item.is_finite()) {
                Some((
                    "ViewBox",
                    TagValue::Array(values.into_iter().map(TagValue::Float).collect()),
                    ValueType::Array,
                ))
            } else {
                Some((
                    "ViewBox",
                    TagValue::String(value.to_owned()),
                    ValueType::String,
                ))
            }
        }
        _ => None,
    }
}

fn add_text_tag(
    metadata: &mut Metadata,
    name: &str,
    text: &str,
    container: &str,
    document_length: u64,
) {
    let value = text.trim();
    if value.is_empty() {
        return;
    }
    metadata.add_tag(Tag {
        namespace: "SVG".to_owned(),
        group: "Document".to_owned(),
        id: None,
        name: name.to_owned(),
        description: Some("SVG document text metadata".to_owned()),
        raw_value: Some(value.as_bytes().to_vec()),
        value: TagValue::String(value.to_owned()),
        value_type: ValueType::String,
        source: Source::new(container, Some(0), Some(document_length)),
        writable: false,
    });
}

fn append_text(
    element: Option<&mut ElementState>,
    text: &str,
    text_bytes: &mut usize,
    limits: ParseLimits,
) -> Result<()> {
    *text_bytes = text_bytes.saturating_add(text.len());
    if *text_bytes > limits.max_value_bytes {
        return Err(MetraError::ResourceLimitExceeded {
            resource: "SVG text".to_owned(),
            limit: limits.max_value_bytes,
        });
    }
    if let Some(element) = element {
        element.text.push_str(text);
    }
    Ok(())
}

fn ensure_node_budget(node_count: &mut usize, limits: ParseLimits) -> Result<()> {
    *node_count = node_count.saturating_add(1);
    if *node_count > limits.max_xmp_nodes {
        Err(MetraError::ResourceLimitExceeded {
            resource: "SVG XML nodes".to_owned(),
            limit: limits.max_xmp_nodes,
        })
    } else {
        Ok(())
    }
}

fn ensure_depth(depth: usize, limits: ParseLimits) -> Result<()> {
    if depth >= limits.max_recursion_depth {
        Err(MetraError::ResourceLimitExceeded {
            resource: "SVG nesting depth".to_owned(),
            limit: limits.max_recursion_depth,
        })
    } else {
        Ok(())
    }
}

fn local_name(bytes: &[u8]) -> String {
    let name = display_name(bytes);
    name.rsplit(':').next().unwrap_or(&name).to_owned()
}

fn display_name(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use metra_core::FileFormat;

    use super::*;

    fn info(size: usize) -> FileInfo {
        FileInfo::new("drawing.svg".into(), size as u64, FileFormat::Svg)
    }

    #[test]
    fn reads_root_dimensions_viewbox_and_document_text() {
        let bytes = br#"<?xml version="1.0"?>
<!-- exported by Metra -->
<svg xmlns="http://www.w3.org/2000/svg" width="640px" height="480px" viewBox="0, 0, 640, 480" version="1.1">
  <title>Sample <tspan>drawing</tspan></title>
  <desc>A bounded vector document.</desc>
</svg>"#;
        let metadata = read_svg(
            &mut Cursor::new(bytes.as_slice()),
            info(bytes.len()),
            ParseLimits::default(),
        )
        .unwrap();

        assert_eq!(metadata.file_info.format, FileFormat::Svg);
        assert_eq!(metadata.find("SVG:Width").unwrap().display_value(), "640px");
        assert_eq!(
            metadata.find("SVG:Height").unwrap().display_value(),
            "480px"
        );
        assert_eq!(
            metadata.find("SVG:ViewBox").unwrap().value,
            TagValue::Array(vec![
                TagValue::Float(0.0),
                TagValue::Float(0.0),
                TagValue::Float(640.0),
                TagValue::Float(480.0),
            ])
        );
        assert_eq!(
            metadata.find("SVG:Title").unwrap().display_value(),
            "Sample drawing"
        );
        assert_eq!(
            metadata.find("SVG:Description").unwrap().display_value(),
            "A bounded vector document."
        );
        assert_eq!(
            metadata.find("SVG:Comment").unwrap().display_value(),
            "exported by Metra"
        );
    }

    #[test]
    fn rejects_doctype_and_enforces_text_limits() {
        let bytes = br#"<!DOCTYPE svg><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        let error = read_svg(
            &mut Cursor::new(bytes.as_slice()),
            info(bytes.len()),
            ParseLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("DOCTYPE"));

        let bytes = br#"<svg><title>&custom;</title></svg>"#;
        let error = read_svg(
            &mut Cursor::new(bytes.as_slice()),
            info(bytes.len()),
            ParseLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported XML entity"));

        let bytes = br#"<svg><title>long title</title></svg>"#;
        let limits = ParseLimits {
            max_value_bytes: 4,
            ..ParseLimits::default()
        };
        let error = read_svg(
            &mut Cursor::new(bytes.as_slice()),
            info(bytes.len()),
            limits,
        )
        .unwrap_err();
        assert!(error.to_string().contains("SVG text"));
    }

    #[test]
    fn extracts_embedded_xmp_from_metadata_element() {
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg"><metadata><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="urn:dc" dc:format="image/svg+xml"/></rdf:RDF></x:xmpmeta></metadata></svg>"#;
        let metadata = read_svg(
            &mut Cursor::new(bytes.as_slice()),
            info(bytes.len()),
            ParseLimits::default(),
        )
        .expect("SVG with embedded XMP should parse");

        let packet = metadata
            .find("XMP:Packet")
            .expect("XMP packet should be read");
        assert_eq!(packet.source.container, "SVG/XMP");
        assert_eq!(packet.source.offset, Some(50));
        assert_eq!(
            metadata.find("XMP:dc:format").unwrap().display_value(),
            "image/svg+xml"
        );
    }
}
